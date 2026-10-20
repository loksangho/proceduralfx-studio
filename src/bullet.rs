// RBD Bullet Solver: rigid pieces held by breakable glue (rapier, Bullet-style).
//
// Inputs, matching Material Fracture's outputs plus a collider:
//   1. geometry: render pieces, by `piece` (one body per value),
//   2. constraints: two-point lines whose ends name their pieces
//      (`constraint_piece`) with a `strength`: fixed joints between the pieces
//      that break when the force through them goes over it,
//   3. proxy: the collision shape of each piece (by `piece`); without it,
//      each piece's convex hull,
//   4. collision: static geometry everything collides with (optional).
// Outputs: the moved geometry, the constraints still holding (their ends
// moving with their pieces), and the moved proxy.

use super::{Mesh, Vec3};
use crate::fracture::CONSTRAINT_PIECE;
use crate::sims::{run, step_count, SIM_FPS};
use rapier3d::math::{Pose, Vector as RVec};
use rapier3d::prelude::{ColliderBuilder, FixedJointBuilder, ImpulseJointHandle, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct BulletSettings {
    pub density: f32,
    pub friction: f32,
    pub bounce: f32,
    pub gravity: f32,
    pub substeps: usize,
    pub ground: bool,
    pub ground_height: f32,
    // Multiplies every constraint's strength (0: glue breaks at once).
    pub glue_scale: f32,
    pub velocity: [f32; 3],
}

impl BulletSettings {
    pub fn defaults() -> Self {
        BulletSettings { density: 1.0, friction: 0.6, bounce: 0.2, gravity: 9.81, substeps: 4, ground: true, ground_height: 0.0, glue_scale: 1.0, velocity: [0.0; 3] }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.density,
            self.friction,
            self.bounce,
            self.gravity,
            self.substeps as f32,
            self.ground as u8 as f32,
            self.ground_height,
            self.glue_scale,
            self.velocity[0],
            self.velocity[1],
            self.velocity[2],
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        BulletSettings {
            density: g(0, d.density).max(0.001),
            friction: g(1, d.friction).max(0.0),
            bounce: g(2, d.bounce).clamp(0.0, 1.0),
            gravity: g(3, d.gravity),
            substeps: (g(4, d.substeps as f32).max(1.0) as usize).min(32),
            ground: g(5, 1.0) > 0.5,
            ground_height: g(6, 0.0),
            glue_scale: g(7, 1.0).max(0.0),
            velocity: [g(8, 0.0), g(9, 0.0), g(10, 0.0)],
        }
    }
}

// RBD Configure: physical properties for pieces (all, or those in the node's group).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigureSettings {
    pub density: f32,
    pub friction: f32,
    pub bounce: f32,
    pub active: bool,
    pub velocity: [f32; 3],
    // Write each property only when on.
    pub set_density: bool,
    pub set_friction: bool,
    pub set_bounce: bool,
    pub set_active: bool,
    pub set_velocity: bool,
}

impl ConfigureSettings {
    pub fn defaults() -> Self {
        ConfigureSettings {
            density: 1.0,
            friction: 0.6,
            bounce: 0.2,
            active: true,
            velocity: [0.0; 3],
            set_density: true,
            set_friction: true,
            set_bounce: true,
            set_active: true,
            set_velocity: false,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        let b = |v: bool| v as u8 as f32;
        vec![
            self.density,
            self.friction,
            self.bounce,
            b(self.active),
            self.velocity[0],
            self.velocity[1],
            self.velocity[2],
            b(self.set_density),
            b(self.set_friction),
            b(self.set_bounce),
            b(self.set_active),
            b(self.set_velocity),
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        let on = |i: usize, v: bool| g(i, v as u8 as f32) > 0.5;
        ConfigureSettings {
            density: g(0, d.density).max(0.001),
            friction: g(1, d.friction).max(0.0),
            bounce: g(2, d.bounce).clamp(0.0, 1.0),
            active: on(3, true),
            velocity: [g(4, 0.0), g(5, 0.0), g(6, 0.0)],
            set_density: on(7, true),
            set_friction: on(8, true),
            set_bounce: on(9, true),
            set_active: on(10, true),
            set_velocity: on(11, false),
        }
    }
}

// Writes the properties as point attributes on the pieces' points (whole pieces: a
// piece is in the group when any of its points is).
pub fn configure(geo: &Mesh, set: &ConfigureSettings, points: Option<&[bool]>) -> Mesh {
    let n = geo.vertices.len();
    let keys = piece_keys(geo);
    let chosen: std::collections::HashSet<i64> = (0..n).filter(|p| points.is_none_or(|m| m.get(*p).copied().unwrap_or(false))).map(|p| keys[p]).collect();
    let mut out = geo.clone();
    let mut write = |name: &str, width: usize, value: &[f32]| {
        let existing = out.point_attributes.remove(name).filter(|v| v.len() == n * width);
        let mut values = existing.unwrap_or_else(|| match name {
            "active" => vec![1.0; n],
            "density" => vec![1.0; n],
            "friction" => vec![0.6; n],
            "bounce" => vec![0.2; n],
            _ => vec![0.0; n * width],
        });
        for p in 0..n {
            if chosen.contains(&keys[p]) {
                values[p * width..p * width + width].copy_from_slice(value);
            }
        }
        out.point_attributes.insert(name.into(), values);
    };
    if set.set_density {
        write("density", 1, &[set.density]);
    }
    if set.set_friction {
        write("friction", 1, &[set.friction]);
    }
    if set.set_bounce {
        write("bounce", 1, &[set.bounce]);
    }
    if set.set_active {
        write("active", 1, &[set.active as u8 as f32]);
    }
    if set.set_velocity {
        write("v", 3, &set.velocity);
    }
    out
}

fn rv(v: Vec3) -> RVec {
    RVec::new(v.x, v.y, v.z)
}

struct Piece {
    origin: Vec3,
    // From RBD Configure (or the solver's settings): density, friction, bounce, active, initial velocity.
    density: Option<f32>,
    friction: Option<f32>,
    bounce: Option<f32>,
    active: bool,
    velocity: Option<Vec3>,
    geo_points: Vec<usize>,
    proxy_points: Vec<usize>,
    // Collision shape points, relative to the origin.
    hull: Vec<RVec>,
}

struct Glue {
    bodies: [usize; 2],
    // The constraint's two points, and the anchor (their midpoint) relative to each body.
    points: [usize; 2],
    anchors: [Vec3; 2],
    strength: f32,
}

struct Base {
    geo: Mesh,
    constraints: Mesh,
    proxy: Mesh,
    collision: Mesh,
    pieces: Vec<Piece>,
    glue: Vec<Glue>,
}

fn piece_keys(mesh: &Mesh) -> Vec<i64> {
    let n = mesh.vertices.len();
    match mesh.point_attributes.get("piece").filter(|v| v.len() == n) {
        Some(v) => v.iter().map(|p| p.round() as i64).collect(),
        None => vec![0; n],
    }
}

impl Base {
    fn new(geo: Mesh, constraints: Mesh, proxy: Mesh, collision: Mesh) -> Base {
        let keys = piece_keys(&geo);
        let mut order: Vec<i64> = keys.clone();
        order.sort_unstable();
        order.dedup();
        let index: HashMap<i64, usize> = order.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        let mut pieces: Vec<Piece> = order
            .iter()
            .map(|_| Piece { origin: Vec3::zero(), density: None, friction: None, bounce: None, active: true, velocity: None, geo_points: Vec::new(), proxy_points: Vec::new(), hull: Vec::new() })
            .collect();
        for (p, k) in keys.iter().enumerate() {
            pieces[index[k]].geo_points.push(p);
        }
        let proxy_keys = piece_keys(&proxy);
        for (p, k) in proxy_keys.iter().enumerate() {
            if let Some(&i) = index.get(k) {
                pieces[i].proxy_points.push(p);
            }
        }
        // Per-piece properties: the first point of each piece that has them.
        let n_geo = geo.vertices.len();
        let float = |name: &str| geo.point_attributes.get(name).filter(|v| v.len() == n_geo);
        for piece in &mut pieces {
            let Some(&first) = piece.geo_points.first() else { continue };
            piece.density = float("density").map(|v| v[first]);
            piece.friction = float("friction").map(|v| v[first]);
            piece.bounce = float("bounce").map(|v| v[first]);
            piece.active = float("active").is_none_or(|v| v[first] > 0.5);
            piece.velocity = geo.point_attributes.get("v").filter(|v| v.len() == n_geo * 3).map(|v| Vec3::new(v[first * 3], v[first * 3 + 1], v[first * 3 + 2]));
        }
        for piece in &mut pieces {
            piece.origin = piece.geo_points.iter().fold(Vec3::zero(), |s, &p| s.add(geo.vertices[p])).scale(1.0 / piece.geo_points.len().max(1) as f32);
            let shape: Vec<Vec3> = if piece.proxy_points.is_empty() {
                piece.geo_points.iter().map(|&p| geo.vertices[p]).collect()
            } else {
                piece.proxy_points.iter().map(|&p| proxy.vertices[p]).collect()
            };
            piece.hull = shape.iter().map(|p| rv(p.sub(piece.origin))).collect();
        }
        // Constraints: consecutive point pairs of each two-point line.
        let mut glue = Vec::new();
        let n = constraints.vertices.len();
        if let Some(ends) = constraints.point_attributes.get(CONSTRAINT_PIECE).filter(|e| e.len() == n) {
            let strength = constraints.point_attributes.get("strength").filter(|s| s.len() == n);
            for range in crate::groom::curves(&constraints).into_iter().filter(|r| r.len() == 2) {
                let (p0, p1) = (range.start, range.start + 1);
                let (Some(&a), Some(&b)) = (index.get(&(ends[p0].round() as i64)), index.get(&(ends[p1].round() as i64))) else { continue };
                if a == b {
                    continue;
                }
                let mid = constraints.vertices[p0].add(constraints.vertices[p1]).scale(0.5);
                glue.push(Glue {
                    bodies: [a, b],
                    points: [p0, p1],
                    anchors: [mid.sub(pieces[a].origin), mid.sub(pieces[b].origin)],
                    strength: strength.map_or(f32::INFINITY, |s| s[p0]),
                });
            }
        }
        Base { geo, constraints, proxy, collision, pieces, glue }
    }
}

struct State {
    base: Arc<Base>,
    world: PhysicsWorld,
    bodies: Vec<RigidBodyHandle>,
    joints: Vec<Option<ImpulseJointHandle>>,
}

#[derive(Clone)]
struct Snapshot {
    base: Arc<Base>,
    poses: Vec<(Pose, RVec, RVec)>,
    broken: Vec<bool>,
}

fn build(base: &Arc<Base>, set: &BulletSettings, snap: Option<&Snapshot>) -> State {
    let mut world = PhysicsWorld::new();
    world.gravity = RVec::new(0.0, -set.gravity, 0.0);
    world.integration_parameters.dt = 1.0 / (SIM_FPS * set.substeps.clamp(1, 32) as f32);
    if set.ground {
        world.colliders.insert(ColliderBuilder::cuboid(1000.0, 0.5, 1000.0).translation(RVec::new(0.0, set.ground_height - 0.5, 0.0)).friction(set.friction).restitution(set.bounce));
    }
    if !base.collision.indices.is_empty() {
        let vertices = base.collision.vertices.iter().map(|v| rv(*v)).collect();
        let indices = base.collision.indices.iter().map(|t| t.map(|i| i as u32)).collect();
        if let Ok(c) = ColliderBuilder::trimesh(vertices, indices) {
            world.colliders.insert(c.friction(set.friction).restitution(set.bounce));
        }
    }
    let bodies: Vec<RigidBodyHandle> = base
        .pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            // Inactive pieces don't move (others still hit them).
            let body = if piece.active { RigidBodyBuilder::dynamic().ccd_enabled(true) } else { RigidBodyBuilder::fixed() };
            let start_v = piece.velocity.unwrap_or(Vec3::zero()).add(Vec3::from_arr(set.velocity));
            let body = match snap.and_then(|s| s.poses.get(i)) {
                Some((pose, lin, ang)) => body.pose(*pose).linvel(*lin).angvel(*ang),
                None => body.translation(rv(piece.origin)).linvel(rv(start_v)),
            };
            let handle = world.bodies.insert(body);
            let collider = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if piece.hull.len() >= 4 { ColliderBuilder::convex_hull(&piece.hull) } else { None }
            }))
            .ok()
            .flatten()
            .unwrap_or_else(|| ColliderBuilder::ball(piece.hull.iter().map(|p| p.length()).fold(0.05, f32::max)));
            let collider = collider
                .density(piece.density.unwrap_or(set.density).max(0.001))
                .friction(piece.friction.unwrap_or(set.friction))
                .restitution(piece.bounce.unwrap_or(set.bounce));
            world.colliders.insert_with_parent(collider, handle, &mut world.bodies);
            handle
        })
        .collect();
    let joints = base
        .glue
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if snap.is_some_and(|s| s.broken.get(i).copied().unwrap_or(false)) || set.glue_scale <= 0.0 {
                return None;
            }
            let joint = FixedJointBuilder::new().local_anchor1(rv(g.anchors[0])).local_anchor2(rv(g.anchors[1]));
            Some(world.impulse_joints.insert(bodies[g.bodies[0]], bodies[g.bodies[1]], joint, true))
        })
        .collect();
    State { base: base.clone(), world, bodies, joints }
}

fn step(state: &mut State, set: &BulletSettings) {
    state.world.step();
    // Glue breaks when the force through it (impulse per step) goes over its strength.
    let dt = state.world.integration_parameters.dt.max(1e-6);
    for (i, slot) in state.joints.iter_mut().enumerate() {
        let Some(h) = *slot else { continue };
        let Some(joint) = state.world.impulse_joints.get(h) else { continue };
        let imp = joint.impulses;
        let force = (imp[0] * imp[0] + imp[1] * imp[1] + imp[2] * imp[2]).sqrt() / dt;
        if force > state.base.glue[i].strength * set.glue_scale {
            state.world.impulse_joints.remove(h, true);
            *slot = None;
        }
    }
}

fn pose_of(state: &State, i: usize) -> Pose {
    *state.world.bodies[state.bodies[i]].position()
}

fn outputs(state: &State) -> (Mesh, Mesh, Mesh) {
    let base = &state.base;
    let moved = |p: Vec3, piece: usize| {
        let local = rv(p.sub(base.pieces[piece].origin));
        let w = pose_of(state, piece).transform_point(local);
        Vec3::new(w.x, w.y, w.z)
    };
    let mut geo = base.geo.clone();
    let mut proxy = base.proxy.clone();
    let mut velocity = vec![0.0f32; geo.vertices.len() * 3];
    for (i, piece) in base.pieces.iter().enumerate() {
        let body = &state.world.bodies[state.bodies[i]];
        let (lin, ang, centre) = (body.linvel(), body.angvel(), body.position().translation);
        for &p in &piece.geo_points {
            let w = moved(base.geo.vertices[p], i);
            geo.vertices[p] = w;
            let v = lin + ang.cross(rv(w) - centre);
            velocity[p * 3..p * 3 + 3].copy_from_slice(&[v.x, v.y, v.z]);
        }
        for &p in &piece.proxy_points {
            proxy.vertices[p] = moved(base.proxy.vertices[p], i);
        }
    }
    geo.point_attributes.insert("v".into(), velocity);
    geo.point_attributes.remove("N");
    // Constraints still holding, their ends following their pieces.
    let keep: Vec<usize> = (0..base.glue.len()).filter(|i| state.joints[*i].is_some()).collect();
    let mut constraints = Mesh::default();
    let mut picked = Vec::new();
    for &i in &keep {
        let g = &base.glue[i];
        for k in 0..2 {
            // Each end moves with its own piece.
            let w = moved(base.constraints.vertices[g.points[k]], g.bodies[k]);
            constraints.vertices.push(w);
            picked.push(g.points[k]);
        }
    }
    let n_in = base.constraints.vertices.len();
    for (key, values) in &base.constraints.point_attributes {
        if n_in > 0 && values.len() % n_in == 0 && !key.starts_with("detail:") {
            let w = values.len() / n_in;
            constraints.point_attributes.insert(key.clone(), picked.iter().flat_map(|&p| values[p * w..p * w + w].iter().copied()).collect());
        }
    }
    (geo, constraints, proxy)
}

// Returns (geometry, constraints, proxy) at `time`.
pub fn simulate(
    key: u64,
    set: &BulletSettings,
    time: f32,
    inputs: impl FnOnce() -> (Mesh, Mesh, Mesh, Mesh),
) -> (Mesh, Mesh, Mesh) {
    let mut result = (Mesh::default(), Mesh::default(), Mesh::default());
    let packed = run(
        key,
        step_count(time, set.substeps.clamp(1, 32)),
        12,
        || {
            let (geo, constraints, proxy, collision) = inputs();
            build(&Arc::new(Base::new(geo, constraints, proxy, collision)), set, None)
        },
        |s: &State| Snapshot {
            base: s.base.clone(),
            poses: s.bodies.iter().map(|h| {
                let b = &s.world.bodies[*h];
                (*b.position(), b.linvel(), b.angvel())
            }).collect(),
            broken: s.joints.iter().map(Option::is_none).collect(),
        },
        |snap: &Snapshot| build(&snap.base, set, Some(snap)),
        |s, _| step(s, set),
        |s| {
            let (geo, constraints, proxy) = outputs(s);
            // `run` returns one mesh: the other two outputs ride along in side attributes.
            let mut out = merge_out(geo, constraints, proxy);
            out.point_attributes.insert("detail:bullet".into(), vec![0.0]);
            out
        },
    );
    if let Some((g, c, p)) = split_out(&packed) {
        result = (g, c, p);
    }
    result
}

// The three outputs travel through the step cache as one mesh: the geometry,
// with the constraints and proxy stored in side attributes.
fn merge_out(geo: Mesh, constraints: Mesh, proxy: Mesh) -> Mesh {
    let mut out = geo;
    for (tag, m) in [("c", constraints), ("p", proxy)] {
        out.point_attributes.insert(format!("detail:bullet_{tag}_P"), m.vertices.iter().flat_map(|v| [v.x, v.y, v.z]).collect());
        out.point_attributes.insert(format!("detail:bullet_{tag}_T"), m.indices.iter().flat_map(|t| t.map(|i| i as f32)).collect());
        for (k, v) in m.point_attributes {
            out.point_attributes.insert(format!("detail:bullet_{tag}:{k}"), v);
        }
    }
    out
}

fn split_out(m: &Mesh) -> Option<(Mesh, Mesh, Mesh)> {
    m.point_attributes.get("detail:bullet")?;
    let mut geo = m.clone();
    let mut parts = Vec::new();
    for tag in ["c", "p"] {
        let mut part = Mesh::default();
        let prefix = format!("detail:bullet_{tag}");
        if let Some(p) = geo.point_attributes.remove(&format!("{prefix}_P")) {
            part.vertices = p.chunks(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect();
        }
        if let Some(t) = geo.point_attributes.remove(&format!("{prefix}_T")) {
            part.indices = t.chunks(3).map(|c| [c[0] as usize, c[1] as usize, c[2] as usize]).collect();
        }
        let keys: Vec<String> = geo.point_attributes.keys().filter(|k| k.starts_with(&format!("{prefix}:"))).cloned().collect();
        for k in keys {
            let v = geo.point_attributes.remove(&k).unwrap();
            part.point_attributes.insert(k[prefix.len() + 1..].to_owned(), v);
        }
        parts.push(part);
    }
    geo.point_attributes.remove("detail:bullet");
    let proxy = parts.pop()?;
    let constraints = parts.pop()?;
    Some((geo, constraints, proxy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fracture::{fracture_all, FractureSettings};

    fn cube(centre: Vec3, size: f32) -> Mesh {
        let mut m = Mesh::default();
        for k in 0..8 {
            let o = Vec3::new((k & 1) as f32 - 0.5, ((k >> 1) & 1) as f32 - 0.5, ((k >> 2) & 1) as f32 - 0.5);
            m.vertices.push(centre.add(o.scale(size)));
        }
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            m.indices.push([f[0], f[1], f[2]]);
            m.indices.push([f[0], f[2], f[3]]);
        }
        m
    }

    fn spread(m: &Mesh) -> f32 {
        let (lo, hi) = m.bounds().unwrap();
        (hi.x - lo.x).max(hi.z - lo.z)
    }

    #[test]
    fn strong_glue_holds_and_weak_glue_shatters() {
        // A fractured block dropped onto the ground from 3 m.
        let block = cube(Vec3::new(0.0, 3.0, 0.0), 1.0);
        let (geo, glue, proxy) = fracture_all(&block, &Mesh::default(), &Mesh::default(), &FractureSettings { pieces: 8, ..FractureSettings::defaults(0) }).unwrap();
        assert!(!glue.vertices.is_empty());
        let drop = |scale: f32, key: u64| {
            crate::sims::clear_all();
            let set = BulletSettings { glue_scale: scale, ..BulletSettings::defaults() };
            simulate(key, &set, 2.5, || (geo.clone(), glue.clone(), proxy.clone(), Mesh::default()))
        };
        let (held, held_glue, held_proxy) = drop(1e9, 501);
        let (lo, _) = held.bounds().unwrap();
        assert!(lo.y > -0.05 && lo.y < 0.3, "the block lands on the ground: lowest {}", lo.y);
        assert!(spread(&held) < 1.3, "unbreakable glue keeps the block whole: {}", spread(&held));
        assert_eq!(held_glue.vertices.len(), glue.vertices.len(), "no constraint broke");
        assert_eq!(held_proxy.vertices.len(), proxy.vertices.len());
        let (loose, loose_glue, _) = drop(0.0, 502);
        assert!(loose_glue.vertices.is_empty(), "no glue at all");
        assert!(spread(&loose) > spread(&held) + 0.05, "unglued pieces scatter: {} vs {}", spread(&loose), spread(&held));
        assert!(loose.vertices.iter().all(|v| v.y > -0.1));
        assert_eq!(BulletSettings::from_params(&BulletSettings::defaults().to_params()), BulletSettings::defaults());
    }

    #[test]
    fn configure_makes_pieces_static_or_fast() {
        // Two separate cubes as pieces 0 and 1, high in the air.
        let mut a = cube(Vec3::new(-2.0, 3.0, 0.0), 0.5);
        a.point_attributes.insert("piece".into(), vec![0.0; 8]);
        let mut b = cube(Vec3::new(2.0, 3.0, 0.0), 0.5);
        b.point_attributes.insert("piece".into(), vec![1.0; 8]);
        let geo = crate::node_merge(a, b);
        // Piece 0 inactive (only its points are in the group).
        let mask: Vec<bool> = (0..16).map(|p| p < 8).collect();
        let configured = configure(&geo, &ConfigureSettings { active: false, ..ConfigureSettings::defaults() }, Some(&mask));
        assert_eq!(configured.point_attributes["active"][0], 0.0);
        assert_eq!(configured.point_attributes["active"][8], 1.0);
        crate::sims::clear_all();
        let (out, _, _) = simulate(504, &BulletSettings::defaults(), 1.5, || (configured.clone(), Mesh::default(), Mesh::default(), Mesh::default()));
        assert!((out.vertices[0].y - geo.vertices[0].y).abs() < 1e-4, "the inactive piece stays put");
        assert!(out.vertices[8].y < 1.0, "the active one falls");
        let s = ConfigureSettings::defaults();
        assert_eq!(ConfigureSettings::from_params(&s.to_params()), s);
    }

    #[test]
    fn collision_geometry_stops_pieces() {
        crate::sims::clear_all();
        let mut piece = cube(Vec3::new(0.0, 3.0, 0.0), 0.5);
        piece.point_attributes.insert("piece".into(), vec![0.0; 8]);
        // A static table at y = 1, no ground.
        let table = cube(Vec3::new(0.0, 0.75, 0.0), 0.5).clone();
        let set = BulletSettings { ground: false, ..BulletSettings::defaults() };
        let (out, _, _) = simulate(503, &set, 2.0, || (piece.clone(), Mesh::default(), Mesh::default(), table.clone()));
        let (lo, _) = out.bounds().unwrap();
        assert!((lo.y - 1.0).abs() < 0.1, "rests on the table top at y=1: {}", lo.y);
    }
}
