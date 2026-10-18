// Muscles: shape, contraction, jiggle and skin.
//
// - Muscle builds a muscle body from origin to insertion (or along an input
//   curve): a tube with a fusiform (spindle) or strap (flat) profile. Its
//   activation (0..1, animatable) shortens the belly towards its middle and
//   thickens it so its volume stays about the same, while the tendons at the
//   ends stretch to keep the attachments where they are.
// - Muscle Solver adds secondary motion: every point follows the animated
//   muscle through a damped spring, displacements are smoothed over the
//   surface so it wobbles as a whole, and the tendon ends stay pinned.
// - Muscle Skin moves a skin mesh with the muscles under it: each skin point
//   follows the nearest muscle points' motion away from their rest shape
//   (the muscles at frame 0), fading with distance.
//
// Muscles carry `fiber` (direction along the muscle), `muscle_u` (0 at the
// origin, 1 at the insertion), `muscle` (which muscle) and point groups
// `origin` and `insertion` (tendon tips).

use super::{Mesh, Vec3};
use crate::sims::{run, step_count, SIM_FPS};
use std::collections::HashMap;

pub const PROFILES: [&str; 2] = ["Fusiform (spindle)", "Strap (flat)"];

#[derive(Clone, Debug, PartialEq)]
pub struct MuscleSettings {
    pub origin: [f32; 3],
    pub insertion: [f32; 3],
    pub radius: f32,
    pub profile: usize,
    // Strap muscles: width to thickness.
    pub flatness: f32,
    // Fraction of the length that's tendon at each end.
    pub tendon: f32,
    pub activation: f32,
    // How much the belly shortens at full activation.
    pub contraction: f32,
    pub segments: usize,
    pub sides: usize,
    // Which way the strap's wide side faces, and the muscle's id.
    pub twist: f32,
    pub id: usize,
}

impl MuscleSettings {
    pub fn defaults() -> Self {
        MuscleSettings {
            origin: [0.0, 1.0, 0.0],
            insertion: [0.0, 0.0, 0.0],
            radius: 0.12,
            profile: 0,
            flatness: 3.0,
            tendon: 0.15,
            activation: 0.0,
            contraction: 0.3,
            segments: 24,
            sides: 16,
            twist: 0.0,
            id: 0,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = self.origin.to_vec();
        p.extend_from_slice(&self.insertion);
        p.extend([
            self.radius,
            self.profile as f32,
            self.flatness,
            self.tendon,
            self.activation,
            self.contraction,
            self.segments as f32,
            self.sides as f32,
            self.twist,
            self.id as f32,
        ]);
        p
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        MuscleSettings {
            origin: [g(0, d.origin[0]), g(1, d.origin[1]), g(2, d.origin[2])],
            insertion: [g(3, d.insertion[0]), g(4, d.insertion[1]), g(5, d.insertion[2])],
            radius: g(6, d.radius).max(1e-4),
            profile: (g(7, 0.0).max(0.0) as usize).min(1),
            flatness: g(8, d.flatness).max(1.0),
            tendon: g(9, d.tendon).clamp(0.0, 0.45),
            activation: g(10, d.activation).clamp(0.0, 1.0),
            contraction: g(11, d.contraction).clamp(0.0, 0.8),
            segments: (g(12, d.segments as f32).max(2.0) as usize).min(256),
            sides: (g(13, d.sides as f32).max(3.0) as usize).min(64),
            twist: g(14, 0.0),
            id: g(15, 0.0).max(0.0) as usize,
        }
    }
}

// Point along a polyline at arc-length fraction `t`, with its tangent.
fn along(path: &[Vec3], lengths: &[f32], t: f32) -> (Vec3, Vec3) {
    let total = *lengths.last().unwrap_or(&0.0);
    let s = t.clamp(0.0, 1.0) * total;
    let i = lengths.partition_point(|l| *l < s).clamp(1, path.len() - 1);
    let (a, b) = (path[i - 1], path[i]);
    let seg = (lengths[i] - lengths[i - 1]).max(1e-9);
    let f = ((s - lengths[i - 1]) / seg).clamp(0.0, 1.0);
    (a.add(b.sub(a).scale(f)), b.sub(a).normalize())
}

// Builds a muscle. `curve`: optional path points (in order) replacing origin → insertion.
pub fn muscle(curve: &Mesh, set: &MuscleSettings) -> Result<Mesh, String> {
    let path: Vec<Vec3> = if curve.vertices.len() >= 2 {
        curve.vertices.clone()
    } else {
        vec![Vec3::from_arr(set.origin), Vec3::from_arr(set.insertion)]
    };
    let mut lengths = vec![0.0f32];
    for w in path.windows(2) {
        lengths.push(lengths.last().unwrap() + w[1].sub(w[0]).length());
    }
    if *lengths.last().unwrap() < 1e-5 {
        return Err("the origin and insertion are in the same place".into());
    }
    let (rows, sides) = (set.segments.max(2), set.sides.max(3));
    // Contraction: the belly (between the tendons) shrinks towards its centre and
    // thickens by 1/sqrt(length factor) to keep its volume.
    let shorten = 1.0 - set.contraction * set.activation;
    let tendon = set.tendon.clamp(0.0, 0.45);
    let belly_half = (0.5 - tendon) * shorten;
    let (b0, b1) = (0.5 - belly_half, 0.5 + belly_half);
    let thicken = 1.0 / shorten.sqrt();
    // A frame along the path: start with any perpendicular and carry it along (parallel transport).
    let (_, t0) = along(&path, &lengths, 0.0);
    let helper = if t0.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
    let mut normal = t0.cross(helper).normalize();
    let (sin_t, cos_t) = set.twist.to_radians().sin_cos();
    let mut mesh = Mesh::default();
    let (mut fiber, mut us) = (Vec::new(), Vec::new());
    let mut prev_t = t0;
    for r in 0..=rows {
        let u = r as f32 / rows as f32;
        let (centre, tangent) = along(&path, &lengths, u);
        // Rotate the frame by the change in tangent.
        let axis = prev_t.cross(tangent);
        if axis.length() > 1e-6 {
            normal = normal.sub(tangent.scale(normal.dot(tangent))).normalize();
        }
        prev_t = tangent;
        let binormal = tangent.cross(normal);
        let (n1, n2) = (normal.scale(cos_t).add(binormal.scale(sin_t)), binormal.scale(cos_t).sub(normal.scale(sin_t)));
        // Thickness: thin tendons at the ends, the belly's profile in between.
        let thickness = if u <= b0 || u >= b1 {
            0.18
        } else {
            let b = (u - b0) / (b1 - b0);
            0.18 + 0.82 * (std::f32::consts::PI * b).sin().powf(0.7)
        };
        let belly = if u > b0 && u < b1 { thicken } else { 1.0 };
        let radius = set.radius * thickness * belly;
        let (wide, thin) = if set.profile == 1 { (radius * set.flatness.sqrt(), radius / set.flatness.sqrt()) } else { (radius, radius) };
        for s in 0..sides {
            let a = s as f32 / sides as f32 * std::f32::consts::TAU;
            mesh.vertices.push(centre.add(n1.scale(a.cos() * wide)).add(n2.scale(a.sin() * thin)));
            fiber.extend([tangent.x, tangent.y, tangent.z]);
            us.push(u);
        }
    }
    for r in 0..rows {
        for s in 0..sides {
            let (a, b) = (r * sides + s, r * sides + (s + 1) % sides);
            let (c, d) = (a + sides, b + sides);
            mesh.indices.push([a, b, d]);
            mesh.indices.push([a, d, c]);
        }
    }
    // Caps at the tendon tips.
    for (row, flip) in [(0, true), (rows, false)] {
        let (centre, tangent) = along(&path, &lengths, row as f32 / rows as f32);
        let c = mesh.vertices.len();
        mesh.vertices.push(centre);
        fiber.extend([tangent.x, tangent.y, tangent.z]);
        us.push(row as f32 / rows as f32);
        for s in 0..sides {
            let (a, b) = (row * sides + s, row * sides + (s + 1) % sides);
            mesh.indices.push(if flip { [c, b, a] } else { [c, a, b] });
        }
    }
    let n = mesh.vertices.len();
    mesh.point_attributes.insert("group:origin".into(), us.iter().map(|u| (*u <= 1e-6) as u8 as f32).collect());
    mesh.point_attributes.insert("group:insertion".into(), us.iter().map(|u| (*u >= 1.0 - 1e-6) as u8 as f32).collect());
    mesh.point_attributes.insert("fiber".into(), fiber);
    mesh.point_attributes.insert("muscle_u".into(), us);
    mesh.point_attributes.insert("muscle".into(), vec![set.id as f32; n]);
    mesh.point_attributes.insert("activation".into(), vec![set.activation; n]);
    Ok(mesh)
}

// --- Muscle Solver (jiggle) --------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JiggleSettings {
    pub stiffness: f32,
    pub damping: f32,
    // Smoothing passes over the displacement: higher wobbles more as one piece.
    pub smoothing: usize,
    pub substeps: usize,
}

impl JiggleSettings {
    pub fn defaults() -> Self {
        JiggleSettings { stiffness: 120.0, damping: 6.0, smoothing: 4, substeps: 4 }
    }
}

#[derive(Clone, Default)]
struct JiggleState {
    p: Vec<Vec3>,
    v: Vec<Vec3>,
}

fn neighbours(mesh: &Mesh) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); mesh.vertices.len()];
    for t in &mesh.indices {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if !adj[a].contains(&b) {
                adj[a].push(b);
            }
            if !adj[b].contains(&a) {
                adj[b].push(a);
            }
        }
    }
    adj
}

pub fn jiggle(key: u64, set: &JiggleSettings, time: f32, mut target_at: impl FnMut(f32) -> Mesh) -> Mesh {
    let substeps = set.substeps.clamp(1, 16);
    let dt = 1.0 / (SIM_FPS * substeps as f32);
    let mut frame_target: Option<(usize, Mesh, Vec<Vec<usize>>)> = None;
    let positions = run(
        key,
        step_count(time, substeps),
        substeps * 12,
        JiggleState::default,
        Clone::clone,
        Clone::clone,
        |state, at| {
            let frame = at.div_ceil(substeps);
            if frame_target.as_ref().is_none_or(|(f, ..)| *f != frame) {
                let target = target_at(frame as f32 / SIM_FPS);
                let adj = neighbours(&target);
                frame_target = Some((frame, target, adj));
            }
            let (_, target, adj) = frame_target.as_ref().unwrap();
            let n = target.vertices.len();
            if state.p.len() != n {
                // Topology changed (or the first step): start at rest on the target.
                state.p = target.vertices.clone();
                state.v = vec![Vec3::zero(); n];
                return;
            }
            let pinned = |i: usize| {
                ["group:origin", "group:insertion"].iter().any(|g| target.point_attributes.get(*g).is_some_and(|m| m.get(i).is_some_and(|v| *v > 0.5)))
            };
            for i in 0..n {
                let pull = target.vertices[i].sub(state.p[i]).scale(set.stiffness);
                state.v[i] = state.v[i].add(pull.scale(dt)).scale((1.0 - set.damping * dt).max(0.0));
            }
            // Smooth the velocities so the muscle wobbles as a body, not as loose points
            // (pinned tips stay out of it, or they'd hold their neighbours back).
            let pins: Vec<bool> = (0..n).map(pinned).collect();
            for _ in 0..set.smoothing.min(50) {
                let prev = state.v.clone();
                for i in 0..n {
                    let free: Vec<usize> = adj[i].iter().copied().filter(|j| !pins[*j]).collect();
                    if pins[i] || free.is_empty() {
                        continue;
                    }
                    let avg = free.iter().fold(Vec3::zero(), |s, j| s.add(prev[*j])).scale(1.0 / free.len() as f32);
                    state.v[i] = prev[i].scale(0.5).add(avg.scale(0.5));
                }
            }
            for i in 0..n {
                if pins[i] {
                    state.p[i] = target.vertices[i];
                    state.v[i] = Vec3::zero();
                } else {
                    // A slow direct pull settles what smoothing can't (sharp, fine-scale offsets).
                    let settle = 1.0 - (-3.0 * dt).exp();
                    state.p[i] = state.p[i].add(state.v[i].scale(dt));
                    state.p[i] = state.p[i].add(target.vertices[i].sub(state.p[i]).scale(settle));
                }
            }
        },
        |state| Mesh { vertices: state.p.clone(), ..Mesh::default() },
    );
    // The muscle at this time supplies topology and attributes; the solver, positions.
    let mut mesh = match frame_target {
        Some((frame, target, _)) if frame == step_count(time, substeps).div_ceil(substeps) => target,
        _ => target_at(time),
    };
    if positions.vertices.len() == mesh.vertices.len() {
        mesh.vertices = positions.vertices;
    }
    mesh
}

// --- Muscle Skin ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct SkinSettings {
    // Skin points further than this from every muscle don't move.
    pub falloff: f32,
    // Muscle points each skin point follows.
    pub influences: usize,
}

impl SkinSettings {
    pub fn defaults() -> Self {
        SkinSettings { falloff: 0.3, influences: 4 }
    }
}

// Moves `skin` with the muscles' motion from `rest` to `deformed` (same points).
pub fn skin(skin: &Mesh, rest: &Mesh, deformed: &Mesh, set: &SkinSettings) -> Result<Mesh, String> {
    if rest.vertices.len() != deformed.vertices.len() {
        return Err("the muscles changed point count since frame 0".into());
    }
    if rest.vertices.is_empty() {
        return Ok(skin.clone());
    }
    let falloff = set.falloff.max(1e-4);
    let k = set.influences.clamp(1, 16);
    let mut cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    let key = |p: Vec3| [(p.x / falloff).floor() as i32, (p.y / falloff).floor() as i32, (p.z / falloff).floor() as i32];
    for (i, p) in rest.vertices.iter().enumerate() {
        cells.entry(key(*p)).or_default().push(i);
    }
    let mut out = skin.clone();
    for v in out.vertices.iter_mut() {
        let c = key(*v);
        let mut near: Vec<(f32, usize)> = Vec::new();
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    for &j in cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                        let d = rest.vertices[j].sub(*v).length();
                        if d < falloff {
                            near.push((d, j));
                        }
                    }
                }
            }
        }
        if near.is_empty() {
            continue;
        }
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        near.truncate(k);
        let (mut moved, mut total) = (Vec3::zero(), 0.0f32);
        for (d, j) in &near {
            let w = 1.0 / (d + falloff * 0.05);
            moved = moved.add(deformed.vertices[*j].sub(rest.vertices[*j]).scale(w));
            total += w;
        }
        // Full motion at the muscle, fading out to the falloff distance.
        let fade = 1.0 - (near[0].0 / falloff).clamp(0.0, 1.0);
        let fade = fade * fade * (3.0 - 2.0 * fade);
        *v = v.add(moved.scale(fade / total));
    }
    out.point_attributes.remove("N");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(mesh: &Mesh) -> f32 {
        mesh.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            a.dot(b.cross(c)) / 6.0
        }).sum::<f32>().abs()
    }

    fn widest(mesh: &Mesh) -> f32 {
        mesh.vertices.iter().map(|p| (p.x * p.x + p.z * p.z).sqrt()).fold(0.0, f32::max)
    }

    #[test]
    fn activation_bulges_and_keeps_volume_and_attachments() {
        let relaxed = muscle(&Mesh::default(), &MuscleSettings::defaults()).unwrap();
        let tense = muscle(&Mesh::default(), &MuscleSettings { activation: 1.0, ..MuscleSettings::defaults() }).unwrap();
        assert!(widest(&tense) > widest(&relaxed) * 1.1, "{} vs {}", widest(&tense), widest(&relaxed));
        // Volume within 20% (the tendons are thin, so most of it is the belly).
        let (vr, vt) = (volume(&relaxed), volume(&tense));
        assert!((vt - vr).abs() / vr < 0.2, "volume {vr} -> {vt}");
        // The ends stay attached.
        for g in ["group:origin", "group:insertion"] {
            let ends = |m: &Mesh| m.vertices.iter().zip(&m.point_attributes[g]).filter(|(_, v)| **v > 0.5).map(|(p, _)| p.y).sum::<f32>();
            assert!((ends(&relaxed) - ends(&tense)).abs() < 1e-4);
        }
        assert_eq!(relaxed.point_attributes["fiber"].len(), relaxed.vertices.len() * 3);
        // Along a curve, and strap-shaped.
        let curve = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.0), Vec3::new(1.0, 0.5, 0.0)], ..Mesh::default() };
        let strap = muscle(&curve, &MuscleSettings { profile: 1, ..MuscleSettings::defaults() }).unwrap();
        assert!(!strap.indices.is_empty());
        assert!(muscle(&Mesh::default(), &MuscleSettings { insertion: [0.0, 1.0, 0.0], ..MuscleSettings::defaults() }).is_err());
        assert_eq!(MuscleSettings::from_params(&MuscleSettings::defaults().to_params()), MuscleSettings::defaults());
    }

    #[test]
    fn jiggle_overshoots_then_settles() {
        crate::sims::clear_all();
        // The muscle flexes suddenly at t = 0.25 s; the jiggle lags, overshoots, then settles.
        let target = |t: f32| muscle(&Mesh::default(), &MuscleSettings { activation: if t >= 0.25 { 1.0 } else { 0.0 }, ..MuscleSettings::defaults() }).unwrap();
        let set = JiggleSettings::defaults();
        let mid = jiggle(91, &set, 0.30, target);
        let settled = jiggle(91, &set, 3.0, target);
        let goal = target(3.0);
        let err = |m: &Mesh| m.vertices.iter().zip(&goal.vertices).map(|(a, b)| a.sub(*b).length()).fold(0.0, f32::max);
        assert!(err(&mid) > 0.005, "should lag behind the sudden flex: {}", err(&mid));
        assert!(err(&settled) < 1e-3, "should settle, off by {}", err(&settled));
        // Pinned tendon tips never move off the target.
        let pins = &mid.point_attributes["group:origin"];
        let tense = target(0.3);
        for ((p, q), pin) in mid.vertices.iter().zip(&tense.vertices).zip(pins) {
            if *pin > 0.5 {
                assert!(p.sub(*q).length() < 1e-6);
            }
        }
    }

    #[test]
    fn skin_follows_bulging_muscles() {
        let rest = muscle(&Mesh::default(), &MuscleSettings::defaults()).unwrap();
        let flexed = muscle(&Mesh::default(), &MuscleSettings { activation: 1.0, ..MuscleSettings::defaults() }).unwrap();
        // A skin cylinder slightly outside the relaxed belly, and a far-away point.
        let mut skin_mesh = Mesh::default();
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            skin_mesh.vertices.push(Vec3::new(a.cos() * 0.15, 0.5, a.sin() * 0.15));
        }
        skin_mesh.vertices.push(Vec3::new(5.0, 0.5, 0.0));
        let out = skin(&skin_mesh, &rest, &flexed, &SkinSettings::defaults()).unwrap();
        let r = |p: Vec3| (p.x * p.x + p.z * p.z).sqrt();
        assert!(r(out.vertices[0]) > r(skin_mesh.vertices[0]) + 0.005, "skin should bulge out");
        assert_eq!(out.vertices[16], skin_mesh.vertices[16], "far skin stays put");
    }
}
