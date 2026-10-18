// Vellum: an XPBD (extended position-based dynamics) solver in the style of
// Houdini's, for cloth, hair, soft bodies and grains.
//
// Each substep predicts positions from velocity and gravity, then iterates
// constraints (stretch and bend distances, shape matching), collisions
// (collider, ground, particle-particle) and pins, and derives the new velocity
// from how far each point moved. Constraints use XPBD compliance, so their
// stiffness doesn't depend on the substep or iteration counts.

use crate::groom;
use crate::sims::{collider_field, run, step_count, SIM_FPS};
use crate::volume::Volume;
use crate::{Mesh, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

pub const VELLUM_TYPES: [&str; 4] = ["Cloth", "Hair", "Softbody", "Grains"];

#[derive(Clone, Debug, PartialEq)]
pub struct VellumSettings {
    // Index into VELLUM_TYPES.
    pub kind: usize,
    // Resistance to stretching and bending (higher is stiffer; compliance = 1 / stiffness).
    pub stretch_stiffness: f32,
    pub bend_stiffness: f32,
    // Softbody: how strongly each piece springs back to its rest shape each substep (0..1).
    pub shape_stiffness: f32,
    // Mass per point, unless the input has a mass attribute.
    pub mass: f32,
    // Collision radius of each point (grains use pscale when present).
    pub thickness: f32,
    pub gravity: f32,
    pub damping: f32,
    pub friction: f32,
    pub substeps: usize,
    pub iterations: usize,
    pub ground: bool,
    pub self_collision: bool,
    // Pinned points (f@pin > 0.5, and hair roots) follow the animated input instead of staying put.
    pub pin_to_animation: bool,
}

impl VellumSettings {
    pub fn defaults(kind: usize) -> Self {
        VellumSettings {
            kind,
            stretch_stiffness: 1e5,
            bend_stiffness: if kind == 1 { 50.0 } else { 1.0 },
            shape_stiffness: 0.5,
            mass: 1.0,
            thickness: if kind == 3 { 0.05 } else { 0.02 },
            gravity: -9.8,
            damping: 0.1,
            friction: 0.3,
            // Many small substeps converge far better than many iterations
            // (Macklin et al., "Small Steps in Physics Simulation").
            substeps: 16,
            iterations: 4,
            ground: true,
            self_collision: kind == 3,
            pin_to_animation: true,
        }
    }
}

#[derive(Clone, Copy)]
struct Distance {
    a: usize,
    b: usize,
    rest: f32,
    compliance: f32,
}

// The parts of the simulation that never change: rest shape, constraints, pins.
struct Model {
    rest: Vec<Vec3>,
    inv_mass: Vec<f32>,
    radius: Vec<f32>,
    pinned: Vec<bool>,
    distances: Vec<Distance>,
    // Softbody pieces: point indices and their rest positions relative to the rest centre of mass.
    shapes: Vec<(Vec<usize>, Vec<Vec3>)>,
    // Pairs closer than this in the rest pose never self-collide (cloth neighbours).
    self_exclusion: f32,
}

#[derive(Clone)]
struct State {
    x: Vec<Vec3>,
    v: Vec<Vec3>,
    // Warm-started rotation of each softbody piece (quaternion w, x, y, z).
    rotations: Vec<[f32; 4]>,
}

fn unique_edges(mesh: &Mesh) -> Vec<(usize, usize)> {
    let mut edges: Vec<(usize, usize)> = mesh
        .indices
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    edges.sort_unstable();
    edges.dedup();
    edges
}

// Connected pieces of a mesh (by its triangles), each as a list of point indices.
fn components(mesh: &Mesh) -> Vec<Vec<usize>> {
    let n = mesh.vertices.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (a, b) in unique_edges(mesh) {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    out.sort_by_key(|g| g[0]);
    out
}

fn point_attr<'a>(mesh: &'a Mesh, name: &str) -> Option<&'a Vec<f32>> {
    mesh.point_attributes.get(name).filter(|a| a.len() == mesh.vertices.len())
}

fn build_model(mesh: &Mesh, set: &VellumSettings) -> Model {
    let n = mesh.vertices.len();
    let mass = point_attr(mesh, "mass");
    let pin = point_attr(mesh, "pin");
    let pscale = point_attr(mesh, "pscale");
    let mut pinned: Vec<bool> = (0..n).map(|i| pin.is_some_and(|p| p[i] > 0.5)).collect();
    let stretch = 1.0 / set.stretch_stiffness.max(1e-6);
    let bend = 1.0 / set.bend_stiffness.max(1e-6);
    let mut distances = Vec::new();
    let mut add = |a: usize, b: usize, compliance: f32, x: &[Vec3]| {
        if a != b {
            distances.push(Distance { a, b, rest: x[a].sub(x[b]).length(), compliance });
        }
    };
    let x = &mesh.vertices;
    let mut shapes = Vec::new();
    match set.kind {
        // Cloth and softbody: every edge resists stretching.
        0 | 2 => {
            for (a, b) in unique_edges(mesh) {
                add(a, b, stretch, x);
            }
            if set.kind == 0 {
                // Bending: the two points opposite each shared edge keep their distance.
                let mut opposite: HashMap<(usize, usize), usize> = HashMap::new();
                for t in &mesh.indices {
                    for k in 0..3 {
                        let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                        let key = (a.min(b), a.max(b));
                        match opposite.get(&key) {
                            Some(&d) => add(c, d, bend, x),
                            None => {
                                opposite.insert(key, c);
                            }
                        }
                    }
                }
            } else {
                for piece in components(mesh) {
                    let total: f32 = piece.iter().map(|&i| mass.map_or(set.mass, |m| m[i]).max(1e-6)).sum();
                    let centre = piece.iter().fold(Vec3::zero(), |acc, &i| acc.add(x[i].scale(mass.map_or(set.mass, |m| m[i]).max(1e-6))));
                    let centre = centre.scale(1.0 / total);
                    let offsets = piece.iter().map(|&i| x[i].sub(centre)).collect();
                    shapes.push((piece, offsets));
                }
            }
        }
        // Hair: segments resist stretching, every other point resists bending, roots are pinned.
        1 => {
            for range in groom::curves(mesh) {
                pinned[range.start] = true;
                for i in range.start..range.end - 1 {
                    add(i, i + 1, stretch, x);
                    if i + 2 < range.end {
                        add(i, i + 2, bend, x);
                    }
                }
            }
        }
        // Grains: collisions only.
        _ => {}
    }
    let inv_mass = (0..n).map(|i| if pinned[i] { 0.0 } else { 1.0 / mass.map_or(set.mass, |m| m[i]).max(1e-6) }).collect();
    let radius = (0..n).map(|i| if set.kind == 3 { pscale.map_or(set.thickness, |p| p[i]) } else { set.thickness }.max(1e-4)).collect();
    Model {
        rest: x.clone(),
        inv_mass,
        radius,
        pinned,
        distances,
        shapes,
        self_exclusion: set.thickness * 2.5,
    }
}

// --- small quaternion helpers for shape matching -------------------------

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

// Columns of the rotation matrix for unit quaternion `q`.
fn quat_columns(q: [f32; 4]) -> [Vec3; 3] {
    let [w, x, y, z] = q;
    [
        Vec3::new(1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)),
        Vec3::new(2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)),
        Vec3::new(2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)),
    ]
}

// Rotational part of the matrix with columns `a` (Mueller et al. 2016), warm-started from `q`.
fn extract_rotation(a: [Vec3; 3], mut q: [f32; 4]) -> [f32; 4] {
    for _ in 0..24 {
        let r = quat_columns(q);
        let num = r[0].cross(a[0]).add(r[1].cross(a[1])).add(r[2].cross(a[2]));
        let den = (r[0].dot(a[0]) + r[1].dot(a[1]) + r[2].dot(a[2])).abs() + 1e-9;
        let omega = num.scale(1.0 / den);
        let angle = omega.length();
        if angle < 1e-7 {
            break;
        }
        let axis = omega.scale(1.0 / angle);
        let (s, c) = (angle * 0.5).sin_cos();
        q = quat_mul([c, axis.x * s, axis.y * s, axis.z * s], q);
        let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        q = q.map(|v| v / len);
    }
    q
}

// --- the solver -----------------------------------------------------------

fn substep(
    s: &mut State,
    m: &Model,
    set: &VellumSettings,
    dt: f32,
    targets: Option<&[Vec3]>,
    collider: Option<&Volume>,
) {
    let n = s.x.len();
    // Points that start inside the ground or collider are moved out without
    // gaining velocity, so a cloth laid on a surface doesn't jump off it.
    collide_environment(s, m, set, collider, &mut vec![None; n]);
    let previous = s.x.clone();
    // Predict.
    for i in 0..n {
        if m.inv_mass[i] > 0.0 {
            s.v[i].y += set.gravity * dt;
            s.x[i] = s.x[i].add(s.v[i].scale(dt));
        }
    }
    // Pinned points sit on their (possibly animated) targets.
    let pin_targets = |i: usize| targets.and_then(|t| t.get(i).copied()).unwrap_or(m.rest[i]);
    for i in 0..n {
        if m.pinned[i] {
            s.x[i] = pin_targets(i);
        }
    }
    shape_match(s, m, set);
    let mut lambdas = vec![0.0f32; m.distances.len()];
    let dt2 = dt * dt;
    // Per point: the contact normal and how far collisions pushed it out this substep.
    let mut contacts: Vec<Option<(Vec3, f32)>> = vec![None; n];
    for iteration in 0..set.iterations.max(1) {
        // Alternate sweep direction (symmetric Gauss-Seidel): corrections travel
        // along chains both ways, so hanging cloth converges much faster.
        let count = m.distances.len();
        for step in 0..count {
            let k = if iteration % 2 == 0 { step } else { count - 1 - step };
            let c = &m.distances[k];
            let (wa, wb) = (m.inv_mass[c.a], m.inv_mass[c.b]);
            let w = wa + wb;
            if w == 0.0 {
                continue;
            }
            let d = s.x[c.a].sub(s.x[c.b]);
            let len = d.length();
            if len < 1e-9 {
                continue;
            }
            let alpha = c.compliance / dt2;
            let delta = (-(len - c.rest) - alpha * lambdas[k]) / (w + alpha);
            lambdas[k] += delta;
            let n = d.scale(1.0 / len);
            s.x[c.a] = s.x[c.a].add(n.scale(delta * wa));
            s.x[c.b] = s.x[c.b].sub(n.scale(delta * wb));
        }
        // Collisions are projected every iteration so constraints and contacts settle together.
        collide_environment(s, m, set, collider, &mut contacts);
    }
    if set.self_collision {
        self_collide(s, m);
        collide_environment(s, m, set, collider, &mut contacts);
    }
    // Coulomb friction in position space: sliding this substep is cancelled only
    // up to friction x how hard the point was pushed out of the surface, so a
    // strong pull (cloth hanging off an edge) still slides it.
    let mu = set.friction.max(0.0);
    for i in 0..n {
        if let (Some((normal, pushed)), true) = (contacts[i], m.inv_mass[i] > 0.0) {
            let moved = s.x[i].sub(previous[i]);
            let tangent = moved.sub(normal.scale(moved.dot(normal)));
            let slide = tangent.length();
            if slide > 1e-12 {
                let limit = mu * pushed;
                s.x[i] = s.x[i].sub(tangent.scale(if slide <= limit { 1.0 } else { limit / slide }));
            }
        }
    }
    // Velocities from the corrected positions.
    let keep = (1.0 - set.damping.max(0.0) * dt).max(0.0);
    for i in 0..n {
        let v = s.x[i].sub(previous[i]).scale(1.0 / dt);
        s.v[i] = if m.inv_mass[i] > 0.0 { v.scale(keep) } else { v };
    }
}

// Softbody shape matching: pull each piece toward its best-fit rigid rest pose.
fn shape_match(s: &mut State, m: &Model, set: &VellumSettings) {
    for (piece, (indices, offsets)) in m.shapes.iter().enumerate() {
        let weights: Vec<f32> = indices.iter().map(|&i| if m.inv_mass[i] > 0.0 { 1.0 / m.inv_mass[i] } else { 1e3 }).collect();
        let total: f32 = weights.iter().sum();
        let centre = indices.iter().zip(&weights).fold(Vec3::zero(), |acc, (&i, w)| acc.add(s.x[i].scale(*w))).scale(1.0 / total);
        let mut a = [Vec3::zero(); 3];
        for ((&i, q), w) in indices.iter().zip(offsets).zip(&weights) {
            let p = s.x[i].sub(centre).scale(*w);
            a[0] = a[0].add(p.scale(q.x));
            a[1] = a[1].add(p.scale(q.y));
            a[2] = a[2].add(p.scale(q.z));
        }
        s.rotations[piece] = extract_rotation(a, s.rotations[piece]);
        let r = quat_columns(s.rotations[piece]);
        let k = set.shape_stiffness.clamp(0.0, 1.0);
        for (&i, q) in indices.iter().zip(offsets) {
            if m.inv_mass[i] > 0.0 {
                let goal = centre.add(r[0].scale(q.x)).add(r[1].scale(q.y)).add(r[2].scale(q.z));
                s.x[i] = s.x[i].add(goal.sub(s.x[i]).scale(k));
            }
        }
    }
}

// Pushes points out of the collider and above the ground, recording the contact
// normal and accumulating how far each point was pushed.
fn collide_environment(s: &mut State, m: &Model, set: &VellumSettings, collider: Option<&Volume>, contacts: &mut [Option<(Vec3, f32)>]) {
    for i in 0..s.x.len() {
        if m.inv_mass[i] == 0.0 {
            continue;
        }
        let r = m.radius[i];
        if let Some(sdf) = collider {
            let phi = sdf.sample(s.x[i]);
            if phi < r {
                let g = sdf.gradient(s.x[i]);
                if g.length() > 1e-6 {
                    let normal = g.normalize();
                    s.x[i] = s.x[i].add(normal.scale(r - phi));
                    contacts[i] = Some((normal, contacts[i].map_or(0.0, |c| c.1) + (r - phi)));
                }
            }
        }
        if set.ground && s.x[i].y < r {
            let depth = r - s.x[i].y;
            s.x[i].y = r;
            contacts[i] = Some((Vec3::new(0.0, 1.0, 0.0), contacts[i].map_or(0.0, |c| c.1) + depth));
        }
    }
}

// Pushes apart points closer than their radii (a hash grid finds the pairs).
fn self_collide(s: &mut State, m: &Model) {
    let n = s.x.len();
    let cell = m.radius.iter().cloned().fold(0.0, f32::max) * 2.0;
    if cell <= 0.0 || n < 2 {
        return;
    }
    let key = |p: Vec3| [p.x, p.y, p.z].map(|c| (c / cell).floor() as i32);
    let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for i in 0..n {
        grid.entry(key(s.x[i])).or_default().push(i);
    }
    for i in 0..n {
        let k = key(s.x[i]);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(others) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                    for &j in others {
                        if j <= i {
                            continue;
                        }
                        let (wi, wj) = (m.inv_mass[i], m.inv_mass[j]);
                        if wi + wj == 0.0 || m.rest[i].sub(m.rest[j]).length() < m.self_exclusion && !m.distances.is_empty() {
                            continue;
                        }
                        let d = s.x[i].sub(s.x[j]);
                        let len = d.length();
                        let min = m.radius[i] + m.radius[j];
                        if len < min && len > 1e-9 {
                            let push = d.scale((min - len) / len / (wi + wj));
                            s.x[i] = s.x[i].add(push.scale(wi));
                            s.x[j] = s.x[j].sub(push.scale(wj));
                        }
                    }
                }
            }
        }
    }
}

fn output(state: &State, rest: &Mesh) -> Mesh {
    let mut mesh = rest.clone();
    mesh.vertices = state.x.clone();
    mesh.point_attributes.insert("v".into(), state.v.iter().flat_map(|v| [v.x, v.y, v.z]).collect());
    mesh
}

// Lerps the animated input's positions between the frames either side of `t`.
struct PinTrack<F: FnMut(f32) -> Mesh> {
    cook: F,
    frames: Vec<(i64, Vec<Vec3>)>,
}

impl<F: FnMut(f32) -> Mesh> PinTrack<F> {
    fn frame(&mut self, f: i64) -> Vec<Vec3> {
        if let Some((_, p)) = self.frames.iter().find(|(k, _)| *k == f) {
            return p.clone();
        }
        let positions = (self.cook)(f as f32 / SIM_FPS).vertices;
        self.frames.push((f, positions.clone()));
        if self.frames.len() > 3 {
            self.frames.remove(0);
        }
        positions
    }

    fn at(&mut self, t: f32) -> Vec<Vec3> {
        let f = (t * SIM_FPS).floor();
        let alpha = t * SIM_FPS - f;
        let (a, b) = (self.frame(f as i64), self.frame(f as i64 + 1));
        if a.len() != b.len() {
            return a;
        }
        a.iter().zip(&b).map(|(p, q)| p.add(q.sub(*p).scale(alpha))).collect()
    }
}

pub fn vellum(
    key: u64,
    settings: &VellumSettings,
    time: f32,
    animated: bool,
    mut geometry_at: impl FnMut(f32) -> Mesh,
    collider: impl FnOnce() -> Mesh,
) -> Mesh {
    let substeps = settings.substeps.clamp(1, 100);
    let dt = 1.0 / (SIM_FPS * substeps as f32);
    let rest = geometry_at(0.0);
    let model = Arc::new(build_model(&rest, settings));
    let shape_count = model.shapes.len();
    let mut collider = Some(collider);
    let mut field: Option<Option<Arc<Volume>>> = None;
    let follow = animated && settings.pin_to_animation;
    let mut track = PinTrack { cook: &mut geometry_at, frames: Vec::new() };
    let initial = rest.vertices.clone();
    let model_for_step = model.clone();
    run(
        key,
        step_count(time, substeps),
        substeps * 6,
        || State { x: initial.clone(), v: vec![Vec3::zero(); initial.len()], rotations: vec![[1.0, 0.0, 0.0, 0.0]; shape_count] },
        |s: &State| s.clone(),
        |s: &State| s.clone(),
        |state, at| {
            let field = field.get_or_insert_with(|| collider.take().and_then(|c| collider_field(&c())));
            let targets = follow.then(|| track.at(at as f32 * dt)).filter(|t| t.len() == state.x.len());
            substep(state, &model_for_step, settings, dt, targets.as_deref(), field.as_deref());
        },
        |state| output(state, &rest),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sims::clear_all;

    fn cloth(n: usize, size: f32, height: f32) -> Mesh {
        let mut mesh = Mesh::default();
        for z in 0..=n {
            for x in 0..=n {
                mesh.vertices.push(Vec3::new((x as f32 / n as f32 - 0.5) * size, height, (z as f32 / n as f32 - 0.5) * size));
            }
        }
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                mesh.indices.push([i, i + n + 1, i + 1]);
                mesh.indices.push([i + 1, i + n + 1, i + n + 2]);
            }
        }
        mesh
    }

    fn cube(centre: Vec3, size: f32) -> Mesh {
        let mut mesh = Mesh::default();
        for k in 0..8 {
            let o = Vec3::new((k & 1) as f32 - 0.5, ((k >> 1) & 1) as f32 - 0.5, ((k >> 2) & 1) as f32 - 0.5);
            mesh.vertices.push(centre.add(o.scale(size)));
        }
        for f in [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
            mesh.indices.push([f[0], f[1], f[2]]);
            mesh.indices.push([f[0], f[2], f[3]]);
        }
        mesh
    }

    fn max_stretch(rest: &Mesh, now: &Mesh) -> f32 {
        unique_edges(rest)
            .into_iter()
            .map(|(a, b)| {
                let r = rest.vertices[a].sub(rest.vertices[b]).length();
                (now.vertices[a].sub(now.vertices[b]).length() / r - 1.0).abs()
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn cloth_drapes_over_a_collider_without_stretching() {
        clear_all();
        let sheet = cloth(16, 2.0, 1.5);
        let table = cube(Vec3::new(0.0, 0.5, 0.0), 1.0);
        let out = vellum(71, &VellumSettings::defaults(0), 2.0, false, |_| sheet.clone(), || table.clone());
        // The middle rests on the table top (y = 1), the corners hang below it.
        let centre = out.vertices[8 * 17 + 8];
        assert!((centre.y - 1.0).abs() < 0.08, "centre at {centre:?}");
        assert!(out.vertices[0].y < 0.9, "corner at {:?}", out.vertices[0]);
        assert!(max_stretch(&sheet, &out) < 0.08, "stretch {}", max_stretch(&sheet, &out));
        assert!(out.vertices.iter().all(|p| p.y > -1e-3), "nothing falls through the ground");
    }

    #[test]
    fn pinned_cloth_hangs_from_its_pins() {
        clear_all();
        let mut sheet = cloth(10, 1.0, 2.0);
        // Pin the two corners of one edge.
        let mut pin = vec![0.0; sheet.vertices.len()];
        pin[0] = 1.0;
        pin[10] = 1.0;
        sheet.point_attributes.insert("pin".into(), pin);
        // Heavy damping so it has settled (rather than mid-swing) when checked.
        let set = VellumSettings { ground: false, damping: 3.0, ..VellumSettings::defaults(0) };
        let out = vellum(72, &set, 3.0, false, |_| sheet.clone(), Mesh::default);
        assert_eq!(out.vertices[0], sheet.vertices[0]);
        assert_eq!(out.vertices[10], sheet.vertices[10]);
        let far = out.vertices[out.vertices.len() - 1];
        assert!(far.y < 1.2, "the free edge swings down: {far:?}");
        assert!(max_stretch(&sheet, &out) < 0.1);
    }

    #[test]
    fn hair_roots_stay_and_follow_animation() {
        clear_all();
        let skin = cloth(4, 1.0, 1.0);
        let guides = groom::hair_guides(&skin, &groom::GuideSettings { count: 12, length: 0.5, length_variance: 0.0, segments: 5, droop: 0.0, seed: 3 });
        let roots: Vec<usize> = groom::curves(&guides).iter().map(|r| r.start).collect();
        // The input rises 1 unit per second; the roots ride along, the tips fall behind and hang.
        let moving = |t: f32| {
            let mut m = guides.clone();
            for p in &mut m.vertices {
                p.y += t;
            }
            m
        };
        let out = vellum(73, &VellumSettings { ground: false, ..VellumSettings::defaults(1) }, 1.0, true, moving, Mesh::default);
        for &r in &roots {
            assert!((out.vertices[r].y - (guides.vertices[r].y + 1.0)).abs() < 1e-4);
        }
        for range in groom::curves(&out) {
            let len: f32 = out.vertices[range.clone()].windows(2).map(|w| w[1].sub(w[0]).length()).sum();
            assert!((len - 0.5).abs() < 0.05, "hair length {len}");
        }
    }

    #[test]
    fn softbody_keeps_its_shape_on_the_ground() {
        clear_all();
        let body = cube(Vec3::new(0.0, 2.0, 0.0), 1.0);
        let out = vellum(74, &VellumSettings::defaults(2), 3.0, false, |_| body.clone(), Mesh::default);
        let lowest = out.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let highest = out.vertices.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        assert!(lowest > -1e-3 && lowest < 0.1, "rests on the ground: {lowest}");
        assert!((highest - lowest - 1.0).abs() < 0.15, "keeps its size: {}", highest - lowest);
    }

    #[test]
    fn grains_pile_without_overlapping() {
        clear_all();
        let mut grains = Mesh::default();
        for i in 0..64u32 {
            grains.vertices.push(Vec3::new((i % 4) as f32 * 0.09, 0.5 + (i / 16) as f32 * 0.09, ((i / 4) % 4) as f32 * 0.09));
        }
        let set = VellumSettings::defaults(3);
        let out = vellum(75, &set, 2.0, false, |_| grains.clone(), Mesh::default);
        let mut closest = f32::INFINITY;
        for i in 0..out.vertices.len() {
            for j in i + 1..out.vertices.len() {
                closest = closest.min(out.vertices[i].sub(out.vertices[j]).length());
            }
        }
        assert!(closest > set.thickness * 2.0 * 0.85, "closest pair {closest}");
        assert!(out.vertices.iter().all(|p| p.y >= set.thickness - 1e-3));
    }

    #[test]
    fn shape_matching_rotation_is_exact_for_rigid_motion() {
        let q_true = {
            let (s, c) = (0.6f32).sin_cos();
            let axis = Vec3::new(1.0, 2.0, 0.5).normalize();
            [c, axis.x * s, axis.y * s, axis.z * s]
        };
        let r = quat_columns(q_true);
        let offsets = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.3), Vec3::new(-0.4, 0.2, 1.0), Vec3::new(0.3, -0.7, -0.2)];
        let mut a = [Vec3::zero(); 3];
        for q in offsets {
            let p = r[0].scale(q.x).add(r[1].scale(q.y)).add(r[2].scale(q.z));
            a[0] = a[0].add(p.scale(q.x));
            a[1] = a[1].add(p.scale(q.y));
            a[2] = a[2].add(p.scale(q.z));
        }
        let found = quat_columns(extract_rotation(a, [1.0, 0.0, 0.0, 0.0]));
        for k in 0..3 {
            assert!(found[k].sub(r[k]).length() < 1e-4);
        }
    }
}
