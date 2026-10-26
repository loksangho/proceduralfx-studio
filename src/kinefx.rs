// KineFX: skeletons as geometry, posing, IK, procedural motion and skinning.
//
// Like Houdini's KineFX, a skeleton is ordinary points: one per joint, with
// - `parent`: the parent joint's point number (-1 for a root),
// - `transform`: the joint's world orientation as a 3x3 matrix whose rows are
//   its local x, y and z axes (so a Wrangle can edit `3@transform`),
// - `detail:skeleton` marking it (the viewport draws bones), and joint names
//   stored as `detail:joint:<name>` = point number.
// Rotations are applied in a joint's own frame and carry its descendants.
//
// Nodes: Skeleton, Rig Pose, Two Bone IK, Procedural Motion, Skeleton Blend,
// Joint Capture (proximity weights) and Bone Deform (linear blend skinning).

use super::{Mesh, Vec3};

pub const SKELETON: &str = "detail:skeleton";
const NAME_PREFIX: &str = "detail:joint:";

// A 3x3 matrix as rows, used with row vectors: v * M = v.x M[0] + v.y M[1] + v.z M[2].
pub type M3 = [Vec3; 3];

pub const IDENTITY: M3 = [Vec3 { x: 1.0, y: 0.0, z: 0.0 }, Vec3 { x: 0.0, y: 1.0, z: 0.0 }, Vec3 { x: 0.0, y: 0.0, z: 1.0 }];

pub fn vm(v: Vec3, m: &M3) -> Vec3 {
    m[0].scale(v.x).add(m[1].scale(v.y)).add(m[2].scale(v.z))
}

pub fn mm(a: &M3, b: &M3) -> M3 {
    [vm(a[0], b), vm(a[1], b), vm(a[2], b)]
}

pub fn transpose(m: &M3) -> M3 {
    [Vec3::new(m[0].x, m[1].x, m[2].x), Vec3::new(m[0].y, m[1].y, m[2].y), Vec3::new(m[0].z, m[1].z, m[2].z)]
}

// Rotation by `angle` (radians) about unit `axis`, for row vectors.
pub fn axis_angle(axis: Vec3, angle: f32) -> M3 {
    let a = axis.normalize();
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    // Column-vector Rodrigues, transposed for row vectors.
    let col = [
        Vec3::new(c + t * a.x * a.x, t * a.x * a.y - s * a.z, t * a.x * a.z + s * a.y),
        Vec3::new(t * a.x * a.y + s * a.z, c + t * a.y * a.y, t * a.y * a.z - s * a.x),
        Vec3::new(t * a.x * a.z - s * a.y, t * a.y * a.z + s * a.x, c + t * a.z * a.z),
    ];
    transpose(&col)
}

// Euler angles in degrees, applied X, then Y, then Z.
pub fn euler(r: [f32; 3]) -> M3 {
    let x = axis_angle(Vec3::new(1.0, 0.0, 0.0), r[0].to_radians());
    let y = axis_angle(Vec3::new(0.0, 1.0, 0.0), r[1].to_radians());
    let z = axis_angle(Vec3::new(0.0, 0.0, 1.0), r[2].to_radians());
    mm(&mm(&x, &y), &z)
}

// The smallest rotation taking direction `from` to `to`.
pub fn align(from: Vec3, to: Vec3) -> M3 {
    let (f, t) = (from.normalize(), to.normalize());
    let axis = f.cross(t);
    let s = axis.length();
    let c = f.dot(t).clamp(-1.0, 1.0);
    if s < 1e-7 {
        if c > 0.0 {
            return IDENTITY;
        }
        // Opposite: half a turn about any perpendicular.
        let helper = if f.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
        return axis_angle(f.cross(helper), std::f32::consts::PI);
    }
    axis_angle(axis.scale(1.0 / s), s.atan2(c))
}

// Re-orthonormalises rows (after blending).
fn orthonormal(m: &M3) -> M3 {
    let x = m[0].normalize();
    let z = x.cross(m[1]).normalize();
    let y = z.cross(x);
    [x, y, z]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub p: Vec<Vec3>,
    pub m: Vec<M3>,
    pub parent: Vec<i32>,
    pub names: Vec<String>,
}

impl Skeleton {
    pub fn is_skeleton(mesh: &Mesh) -> bool {
        mesh.point_attributes.contains_key(SKELETON) && mesh.point_attributes.get("parent").is_some_and(|p| p.len() == mesh.vertices.len())
    }

    pub fn from_mesh(mesh: &Mesh) -> Result<Skeleton, String> {
        let n = mesh.vertices.len();
        let parent = mesh
            .point_attributes
            .get("parent")
            .filter(|p| p.len() == n)
            .ok_or("the input isn't a skeleton: start from a Skeleton node (Rigging menu)")?;
        let parent: Vec<i32> = parent.iter().map(|p| if *p < 0.0 || *p as usize >= n { -1 } else { *p as i32 }).collect();
        let m = match mesh.point_attributes.get("transform").filter(|t| t.len() == n * 9) {
            Some(t) => t.chunks(9).map(|c| [Vec3::new(c[0], c[1], c[2]), Vec3::new(c[3], c[4], c[5]), Vec3::new(c[6], c[7], c[8])]).collect(),
            None => vec![IDENTITY; n],
        };
        let mut names = vec![String::new(); n];
        for (key, value) in &mesh.point_attributes {
            if let (Some(name), Some(i)) = (key.strip_prefix(NAME_PREFIX), value.first()) {
                if let Some(slot) = names.get_mut(*i as usize) {
                    *slot = name.to_owned();
                }
            }
        }
        for (i, name) in names.iter_mut().enumerate() {
            if name.is_empty() {
                *name = format!("joint{i}");
            }
        }
        Ok(Skeleton { p: mesh.vertices.clone(), m, parent, names })
    }

    pub fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh { vertices: self.p.clone(), ..Mesh::default() };
        mesh.point_attributes.insert("parent".into(), self.parent.iter().map(|p| *p as f32).collect());
        mesh.point_attributes.insert("transform".into(), self.m.iter().flat_map(|m| [m[0].x, m[0].y, m[0].z, m[1].x, m[1].y, m[1].z, m[2].x, m[2].y, m[2].z]).collect());
        mesh.point_attributes.insert(SKELETON.into(), vec![1.0]);
        for (i, name) in self.names.iter().enumerate() {
            mesh.point_attributes.insert(format!("{NAME_PREFIX}{name}"), vec![i as f32]);
        }
        mesh
    }

    // Copies the skeleton attributes onto `base` (keeping other attributes a Wrangle added).
    pub fn into_mesh(&self, base: &Mesh) -> Mesh {
        let mut out = base.clone();
        let fresh = self.to_mesh();
        out.vertices = fresh.vertices;
        out.point_attributes.retain(|k, _| !k.starts_with(NAME_PREFIX));
        out.point_attributes.extend(fresh.point_attributes);
        out
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.names.iter().position(|n| n == name).or_else(|| name.parse::<usize>().ok().filter(|i| *i < self.p.len()))
    }

    pub fn children(&self, j: usize) -> Vec<usize> {
        (0..self.p.len()).filter(|k| self.parent[*k] == j as i32).collect()
    }

    // `j` and everything below it.
    pub fn subtree(&self, j: usize) -> Vec<usize> {
        let mut out = vec![j];
        let mut i = 0;
        while i < out.len() {
            out.extend(self.children(out[i]));
            i += 1;
            if out.len() > self.p.len() {
                break;
            }
        }
        out
    }

    // Applies world rotation `d` about joint `j`'s position to it and its descendants.
    pub fn rotate_world(&mut self, j: usize, d: &M3) {
        let pivot = self.p[j];
        for k in self.subtree(j) {
            self.p[k] = vm(self.p[k].sub(pivot), d).add(pivot);
            self.m[k] = mm(&self.m[k], d);
        }
    }

    // Rotates joint `j` by `r` expressed in its own frame.
    pub fn rotate_local(&mut self, j: usize, r: &M3) {
        let mj = self.m[j];
        let d = mm(&mm(&transpose(&mj), r), &mj);
        self.rotate_world(j, &d);
    }

    // Moves joint `j` and its descendants by `t` in its parent's frame.
    pub fn translate_local(&mut self, j: usize, t: Vec3) {
        let frame = if self.parent[j] >= 0 { self.m[self.parent[j] as usize] } else { IDENTITY };
        let offset = vm(t, &frame);
        for k in self.subtree(j) {
            self.p[k] = self.p[k].add(offset);
        }
    }
}

// --- Skeleton presets ------------------------------------------------------------

pub const PRESETS: [&str; 3] = ["Biped", "Chain", "Tail"];

#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonSettings {
    pub preset: usize,
    pub scale: f32,
    // Chain and tail: joints and total length.
    pub joints: usize,
    pub length: f32,
}

impl SkeletonSettings {
    pub fn defaults() -> Self {
        SkeletonSettings { preset: 0, scale: 1.0, joints: 6, length: 2.0 }
    }
}

pub fn skeleton(set: &SkeletonSettings) -> Mesh {
    let mut sk = Skeleton { p: Vec::new(), m: Vec::new(), parent: Vec::new(), names: Vec::new() };
    let mut add = |name: String, p: Vec3, parent: i32| {
        sk.p.push(p.scale(set.scale));
        sk.m.push(IDENTITY);
        sk.parent.push(parent);
        sk.names.push(name);
        (sk.p.len() - 1) as i32
    };
    match set.preset {
        0 => {
            // A 1.85 m biped in T-pose, facing +z, with Houdini-style names.
            let hips = add("hips".into(), Vec3::new(0.0, 1.0, 0.0), -1);
            let spine1 = add("spine1".into(), Vec3::new(0.0, 1.12, 0.0), hips);
            let spine2 = add("spine2".into(), Vec3::new(0.0, 1.25, 0.0), spine1);
            let chest = add("chest".into(), Vec3::new(0.0, 1.38, 0.0), spine2);
            let neck = add("neck".into(), Vec3::new(0.0, 1.55, 0.0), chest);
            let head = add("head".into(), Vec3::new(0.0, 1.65, 0.0), neck);
            add("head_end".into(), Vec3::new(0.0, 1.85, 0.0), head);
            for (side, s) in [("l", 1.0f32), ("r", -1.0f32)] {
                let clavicle = add(format!("clavicle_{side}"), Vec3::new(0.05 * s, 1.48, 0.0), chest);
                let upper = add(format!("upperarm_{side}"), Vec3::new(0.18 * s, 1.48, 0.0), clavicle);
                let fore = add(format!("forearm_{side}"), Vec3::new(0.45 * s, 1.48, 0.0), upper);
                let hand = add(format!("hand_{side}"), Vec3::new(0.7 * s, 1.48, 0.0), fore);
                add(format!("hand_end_{side}"), Vec3::new(0.8 * s, 1.48, 0.0), hand);
                let thigh = add(format!("thigh_{side}"), Vec3::new(0.1 * s, 0.95, 0.0), hips);
                let shin = add(format!("shin_{side}"), Vec3::new(0.1 * s, 0.52, 0.0), thigh);
                let foot = add(format!("foot_{side}"), Vec3::new(0.1 * s, 0.08, 0.0), shin);
                let toe = add(format!("toe_{side}"), Vec3::new(0.1 * s, 0.02, 0.13), foot);
                add(format!("toe_end_{side}"), Vec3::new(0.1 * s, 0.02, 0.2), toe);
            }
        }
        preset => {
            let n = set.joints.clamp(2, 200);
            let mut parent = -1;
            for i in 0..n {
                let t = i as f32 / (n - 1) as f32 * set.length;
                // Chain: up the y axis; tail: back along -z.
                let p = if preset == 1 { Vec3::new(0.0, t, 0.0) } else { Vec3::new(0.0, 0.0, -t) };
                parent = add(format!("joint{i}"), p, parent);
            }
        }
    }
    sk.to_mesh()
}

// --- Rig Pose -------------------------------------------------------------------

pub const POSE_SLOTS: usize = 8;

// Per slot: rotate (x, y, z degrees) and translate (x, y, z).
#[derive(Clone, Debug, PartialEq)]
pub struct RigPose {
    pub joints: Vec<String>,
    pub values: Vec<[f32; 6]>,
}

impl RigPose {
    pub fn new() -> Self {
        RigPose { joints: vec![String::new(); POSE_SLOTS], values: vec![[0.0; 6]; POSE_SLOTS] }
    }
}

impl Default for RigPose {
    fn default() -> Self {
        Self::new()
    }
}

pub fn rig_pose(input: &Mesh, pose: &RigPose) -> Result<Mesh, String> {
    let mut sk = Skeleton::from_mesh(input)?;
    for (name, v) in pose.joints.iter().zip(&pose.values) {
        if name.trim().is_empty() {
            continue;
        }
        let j = sk.find(name).ok_or_else(|| format!("no joint named `{}`", name.trim()))?;
        if v[3..] != [0.0; 3] {
            sk.translate_local(j, Vec3::new(v[3], v[4], v[5]));
        }
        if v[..3] != [0.0; 3] {
            sk.rotate_local(j, &euler([v[0], v[1], v[2]]));
        }
    }
    Ok(sk.into_mesh(input))
}

// --- Two Bone IK -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct IkSettings {
    pub root: String,
    pub mid: String,
    pub tip: String,
    pub goal: [f32; 3],
    pub pole: [f32; 3],
    pub use_pole: bool,
    pub blend: f32,
    // Keep the tip joint's world orientation (hands and feet stay level).
    pub keep_tip: bool,
}

impl IkSettings {
    pub fn defaults() -> Self {
        IkSettings {
            root: "thigh_l".into(),
            mid: "shin_l".into(),
            tip: "foot_l".into(),
            goal: [0.1, 0.3, 0.3],
            pole: [0.1, 0.5, 1.0],
            use_pole: true,
            blend: 1.0,
            keep_tip: true,
        }
    }
}

pub fn two_bone_ik(input: &Mesh, set: &IkSettings) -> Result<Mesh, String> {
    let mut sk = Skeleton::from_mesh(input)?;
    let joint = |name: &str| sk.find(name).ok_or_else(|| format!("no joint named `{}`", name.trim()));
    let (a, b, c) = (joint(&set.root)?, joint(&set.mid)?, joint(&set.tip)?);
    if sk.parent[b] != a as i32 || sk.parent[c] != b as i32 {
        return Err("the joints must be a chain: mid's parent is root, tip's parent is mid".into());
    }
    let tip_frame = sk.m[c];
    let (pa, pb, pc) = (sk.p[a], sk.p[b], sk.p[c]);
    let (l1, l2) = (pb.sub(pa).length(), pc.sub(pb).length());
    let goal = pc.add(Vec3::from_arr(set.goal).sub(pc).scale(set.blend.clamp(0.0, 1.0)));
    let to_goal = goal.sub(pa);
    let d = to_goal.length().clamp(1e-4, (l1 + l2) * 0.9999);
    let dir = to_goal.normalize();
    // The bend direction: towards the pole, or the way the chain already bends.
    let bend_hint = if set.use_pole { Vec3::from_arr(set.pole).sub(pa) } else { pb.sub(pa.add(pc.sub(pa).scale(0.5))) };
    let mut bend = bend_hint.sub(dir.scale(bend_hint.dot(dir)));
    if bend.length() < 1e-6 {
        let helper = if dir.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
        bend = helper.sub(dir.scale(helper.dot(dir)));
    }
    let bend = bend.normalize();
    let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).sqrt();
    let new_b = pa.add(dir.scale(cos_a * l1)).add(bend.scale(sin_a * l1));
    let new_c = pa.add(dir.scale(d));
    sk.rotate_world(a, &align(pb.sub(pa), new_b.sub(pa)));
    let current_c = sk.p[c].sub(sk.p[b]);
    sk.rotate_world(b, &align(current_c, new_c.sub(sk.p[b])));
    if set.keep_tip {
        // Undo the chain's rotation on the tip (and its children) so it keeps its world orientation.
        let undo = mm(&transpose(&sk.m[c]), &tip_frame);
        sk.rotate_world(c, &undo);
    }
    Ok(sk.into_mesh(input))
}

// --- Procedural Motion --------------------------------------------------------------

pub const MOTIONS: [&str; 3] = ["Walk cycle (biped)", "Wave (chains, tails)", "Idle breathing"];

#[derive(Clone, Debug, PartialEq)]
pub struct MotionSettings {
    pub motion: usize,
    // Cycles per second.
    pub rate: f32,
    pub amplitude: f32,
    // Walk: metres per second the hips travel forward (0: walk on the spot).
    pub travel: f32,
    // Wave: phase lag per joint (radians).
    pub lag: f32,
    pub axis: usize,
}

impl MotionSettings {
    pub fn defaults() -> Self {
        MotionSettings { motion: 0, rate: 1.0, amplitude: 1.0, travel: 1.2, lag: 0.6, axis: 0 }
    }
}

pub fn motion(input: &Mesh, set: &MotionSettings, time: f32) -> Result<Mesh, String> {
    let mut sk = Skeleton::from_mesh(input)?;
    let phase = time * set.rate * std::f32::consts::TAU;
    let amp = set.amplitude;
    let rot = |sk: &mut Skeleton, name: &str, r: [f32; 3]| {
        if let Some(j) = sk.find(name) {
            sk.rotate_local(j, &euler(r));
        }
    };
    match set.motion {
        0 => {
            if sk.find("thigh_l").is_none() {
                return Err("the walk cycle needs the Biped skeleton's joint names (thigh_l, shin_l…)".into());
            }
            let s = phase.sin();
            // Legs swing opposite each other; knees bend on the forward swing; arms counter-swing.
            rot(&mut sk, "hips", [0.0, 6.0 * s * amp, 3.0 * (2.0 * phase).cos() * amp]);
            for (side, sign) in [("l", 1.0f32), ("r", -1.0f32)] {
                let swing = sign * s;
                let knee = ((phase + if sign > 0.0 { 0.0 } else { std::f32::consts::PI }) + 1.3).sin().max(0.0);
                rot(&mut sk, &format!("thigh_{side}"), [-30.0 * swing * amp, 0.0, 0.0]);
                rot(&mut sk, &format!("shin_{side}"), [55.0 * knee * amp, 0.0, 0.0]);
                rot(&mut sk, &format!("foot_{side}"), [-15.0 * swing.min(0.0) * amp, 0.0, 0.0]);
                rot(&mut sk, &format!("upperarm_{side}"), [25.0 * swing * amp, 0.0, -sign * 70.0]);
                rot(&mut sk, &format!("forearm_{side}"), [-20.0 * (1.0 + swing) * 0.5 * amp, 0.0, 0.0]);
            }
            // A bob twice per cycle, and forward travel.
            let hips = sk.find("hips").unwrap_or(0);
            let bob = 0.03 * amp * (2.0 * phase).cos().abs();
            sk.translate_local(hips, Vec3::new(0.0, bob - 0.03 * amp, set.travel * time));
        }
        1 => {
            let axis = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)][set.axis.min(2)];
            // Depth from the root gives each joint its lag.
            let n = sk.p.len();
            for j in 0..n {
                let mut depth = 0;
                let mut k = j;
                while sk.parent[k] >= 0 && depth < n {
                    k = sk.parent[k] as usize;
                    depth += 1;
                }
                if sk.parent[j] < 0 {
                    continue;
                }
                let angle = (15.0 * amp).to_radians() * (phase - depth as f32 * set.lag).sin();
                sk.rotate_local(j, &axis_angle(axis, angle));
            }
        }
        _ => {
            let breath = (phase * 0.5).sin();
            for name in ["spine1", "spine2", "chest"] {
                rot(&mut sk, name, [-1.5 * breath * amp, 0.0, 0.0]);
            }
            rot(&mut sk, "head", [2.0 * (phase * 0.3).sin() * amp, 4.0 * (phase * 0.17).sin() * amp, 0.0]);
            for (side, sign) in [("l", 1.0f32), ("r", -1.0f32)] {
                rot(&mut sk, &format!("upperarm_{side}"), [0.0, 0.0, -sign * (72.0 + 2.0 * breath * amp)]);
            }
        }
    }
    Ok(sk.into_mesh(input))
}

// --- Skeleton Blend -----------------------------------------------------------------

pub fn blend(a: &Mesh, b: &Mesh, weight: f32) -> Result<Mesh, String> {
    let (sa, sb) = (Skeleton::from_mesh(a)?, Skeleton::from_mesh(b)?);
    if sa.p.len() != sb.p.len() {
        return Err("the two skeletons have different joints".into());
    }
    let w = weight.clamp(0.0, 1.0);
    let mut out = sa.clone();
    // Blend local rotations and offsets, then rebuild the world pose from the roots down.
    let local = |s: &Skeleton, j: usize| -> (M3, Vec3) {
        let (pm, pp) = if s.parent[j] >= 0 { (s.m[s.parent[j] as usize], s.p[s.parent[j] as usize]) } else { (IDENTITY, Vec3::zero()) };
        (mm(&s.m[j], &transpose(&pm)), vm(s.p[j].sub(pp), &transpose(&pm)))
    };
    let mut order: Vec<usize> = (0..sa.p.len()).filter(|j| sa.parent[*j] < 0).collect();
    let mut i = 0;
    while i < order.len() {
        order.extend(sa.children(order[i]));
        i += 1;
    }
    for j in order {
        let ((ma, oa), (mb, ob)) = (local(&sa, j), local(&sb, j));
        let m = orthonormal(&[0, 1, 2].map(|r| ma[r].scale(1.0 - w).add(mb[r].scale(w))));
        let o = oa.scale(1.0 - w).add(ob.scale(w));
        let (pm, pp) = if out.parent[j] >= 0 { (out.m[out.parent[j] as usize], out.p[out.parent[j] as usize]) } else { (IDENTITY, Vec3::zero()) };
        out.m[j] = mm(&m, &pm);
        out.p[j] = pp.add(vm(o, &pm));
    }
    Ok(out.into_mesh(a))
}

// --- Joint Capture and Bone Deform ----------------------------------------------------

pub const CAPTURE_INDEX: &str = "capture_index";
pub const CAPTURE_WEIGHT: &str = "capture_weight";
pub const MAX_INFLUENCES: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct CaptureSettings {
    pub influences: usize,
    // Weight falls off as 1 / distance^falloff.
    pub falloff: f32,
    pub visualize: bool,
}

impl CaptureSettings {
    pub fn defaults() -> Self {
        CaptureSettings { influences: 3, falloff: 4.0, visualize: false }
    }
}

fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b.sub(a);
    let t = (p.sub(a).dot(ab) / ab.dot(ab).max(1e-12)).clamp(0.0, 1.0);
    p.sub(a.add(ab.scale(t))).length()
}

// Each point's weights to the nearest bones: a bone runs from a joint to each
// child, and moves with that joint; leaf joints count as points.
pub fn capture(mesh: &Mesh, skeleton: &Mesh, set: &CaptureSettings) -> Result<Mesh, String> {
    let sk = Skeleton::from_mesh(skeleton)?;
    let k = set.influences.clamp(1, MAX_INFLUENCES);
    let children: Vec<Vec<usize>> = (0..sk.p.len()).map(|j| sk.children(j)).collect();
    let mut index = Vec::with_capacity(mesh.vertices.len() * MAX_INFLUENCES);
    let mut weight = Vec::with_capacity(mesh.vertices.len() * MAX_INFLUENCES);
    for p in &mesh.vertices {
        let mut near: Vec<(f32, usize)> = (0..sk.p.len())
            .map(|j| {
                let d = if children[j].is_empty() {
                    p.sub(sk.p[j]).length()
                } else {
                    children[j].iter().map(|c| segment_distance(*p, sk.p[j], sk.p[*c])).fold(f32::MAX, f32::min)
                };
                (d, j)
            })
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        near.truncate(k);
        let ws: Vec<f32> = near.iter().map(|(d, _)| 1.0 / (d + 1e-3).powf(set.falloff.max(0.1))).collect();
        let total: f32 = ws.iter().sum();
        for slot in 0..MAX_INFLUENCES {
            index.push(near.get(slot).map_or(-1.0, |n| n.1 as f32));
            weight.push(ws.get(slot).map_or(0.0, |w| w / total));
        }
    }
    let mut out = mesh.clone();
    if set.visualize {
        // Colour by the strongest joint.
        let colours: Vec<f32> = index.chunks(MAX_INFLUENCES).flat_map(|c| {
            let j = c[0].max(0.0) as u32;
            [0.25 + 0.75 * super::hash01(j, 7), 0.25 + 0.75 * super::hash01(j, 8), 0.25 + 0.75 * super::hash01(j, 9)]
        }).collect();
        out.point_attributes.insert("Cd".into(), colours);
    }
    out.point_attributes.insert(CAPTURE_INDEX.into(), index);
    out.point_attributes.insert(CAPTURE_WEIGHT.into(), weight);
    Ok(out)
}

// Linear blend skinning from the rest skeleton (the capture pose) to the animated one.
pub fn bone_deform(mesh: &Mesh, rest: &Mesh, animated: &Mesh) -> Result<Mesh, String> {
    let n = mesh.vertices.len();
    let index = mesh.point_attributes.get(CAPTURE_INDEX).filter(|v| v.len() == n * MAX_INFLUENCES);
    let weight = mesh.point_attributes.get(CAPTURE_WEIGHT).filter(|v| v.len() == n * MAX_INFLUENCES);
    let (Some(index), Some(weight)) = (index, weight) else {
        return Err("the mesh has no capture weights: put a Joint Capture before Bone Deform".into());
    };
    let (rest, anim) = (Skeleton::from_mesh(rest)?, Skeleton::from_mesh(animated)?);
    if rest.p.len() != anim.p.len() {
        return Err("the rest and animated skeletons have different joints".into());
    }
    // Per joint: rest-to-animated as v * R + t.
    let xforms: Vec<(M3, Vec3)> = (0..rest.p.len())
        .map(|j| {
            let r = mm(&transpose(&rest.m[j]), &anim.m[j]);
            (r, anim.p[j].sub(vm(rest.p[j], &r)))
        })
        .collect();
    let mut out = mesh.clone();
    for (i, v) in out.vertices.iter_mut().enumerate() {
        let p = *v;
        let mut moved = Vec3::zero();
        let mut total = 0.0;
        for slot in 0..MAX_INFLUENCES {
            let (j, w) = (index[i * MAX_INFLUENCES + slot], weight[i * MAX_INFLUENCES + slot]);
            if j < 0.0 || w <= 0.0 || j as usize >= xforms.len() {
                continue;
            }
            let (r, t) = &xforms[j as usize];
            moved = moved.add(vm(p, r).add(*t).scale(w));
            total += w;
        }
        if total > 0.0 {
            *v = moved.scale(1.0 / total);
        }
    }
    out.point_attributes.remove("N");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.sub(b).length() < 1e-4
    }

    #[test]
    fn skeletons_round_trip_and_name_their_joints() {
        let mesh = skeleton(&SkeletonSettings::defaults());
        assert!(Skeleton::is_skeleton(&mesh));
        let sk = Skeleton::from_mesh(&mesh).unwrap();
        assert_eq!(sk.p.len(), 27);
        assert_eq!(sk.find("hips"), Some(0));
        let shin = sk.find("shin_l").unwrap();
        assert_eq!(sk.parent[shin], sk.find("thigh_l").unwrap() as i32);
        assert_eq!(Skeleton::from_mesh(&sk.to_mesh()).unwrap(), sk);
        let chain = Skeleton::from_mesh(&skeleton(&SkeletonSettings { preset: 1, joints: 5, length: 4.0, ..SkeletonSettings::defaults() })).unwrap();
        assert!(close(chain.p[4], Vec3::new(0.0, 4.0, 0.0)) && chain.parent == vec![-1, 0, 1, 2, 3]);
    }

    #[test]
    fn rig_pose_rotates_subtrees_in_the_joint_frame() {
        let chain = skeleton(&SkeletonSettings { preset: 1, joints: 3, length: 2.0, ..SkeletonSettings::defaults() });
        let mut pose = RigPose::new();
        pose.joints[0] = "joint1".into();
        pose.values[0] = [0.0, 0.0, -90.0, 0.0, 0.0, 0.0];
        let out = Skeleton::from_mesh(&rig_pose(&chain, &pose).unwrap()).unwrap();
        // joint2 swings from (0, 2, 0) to (1, 1, 0) around joint1 at (0, 1, 0).
        assert!(close(out.p[2], Vec3::new(1.0, 1.0, 0.0)), "{:?}", out.p[2]);
        assert!(close(out.p[1], Vec3::new(0.0, 1.0, 0.0)));
        // Rotating joint1 again about its (now rotated) z turns joint2 further.
        let twice = Skeleton::from_mesh(&rig_pose(&rig_pose(&chain, &pose).unwrap(), &pose).unwrap()).unwrap();
        assert!(close(twice.p[2], Vec3::new(0.0, 0.0, 0.0)), "{:?}", twice.p[2]);
        pose.joints[1] = "nope".into();
        assert!(rig_pose(&chain, &pose).is_err());
    }

    #[test]
    fn two_bone_ik_reaches_the_goal_with_fixed_bone_lengths() {
        let biped = skeleton(&SkeletonSettings::defaults());
        let set = IkSettings::defaults();
        let out = Skeleton::from_mesh(&two_bone_ik(&biped, &set).unwrap()).unwrap();
        let before = Skeleton::from_mesh(&biped).unwrap();
        let (a, b, c) = (before.find("thigh_l").unwrap(), before.find("shin_l").unwrap(), before.find("foot_l").unwrap());
        assert!(close(out.p[c], Vec3::from_arr(set.goal)), "foot at {:?}", out.p[c]);
        let len = |s: &Skeleton, x: usize, y: usize| s.p[x].sub(s.p[y]).length();
        assert!((len(&out, a, b) - len(&before, a, b)).abs() < 1e-4 && (len(&out, b, c) - len(&before, b, c)).abs() < 1e-4);
        // The knee bends towards the pole (forwards, +z).
        assert!(out.p[b].z > 0.05, "knee {:?}", out.p[b]);
        // The foot keeps its orientation, and the toe stays attached.
        assert!(close(out.m[c][2], before.m[c][2]));
        let toe = before.find("toe_l").unwrap();
        assert!((len(&out, c, toe) - len(&before, c, toe)).abs() < 1e-4);
        // Out of reach: the leg points straight at the goal.
        let far = Skeleton::from_mesh(&two_bone_ik(&biped, &IkSettings { goal: [0.1, -5.0, 0.0], ..set.clone() }).unwrap()).unwrap();
        assert!((len(&far, a, c) - (len(&before, a, b) + len(&before, b, c))).abs() < 1e-3);
        assert!(two_bone_ik(&biped, &IkSettings { mid: "hand_l".into(), ..set }).is_err());
    }

    #[test]
    fn walk_and_wave_animate_and_blend_mixes_poses() {
        let biped = skeleton(&SkeletonSettings::defaults());
        let set = MotionSettings::defaults();
        let a = Skeleton::from_mesh(&motion(&biped, &set, 0.25).unwrap()).unwrap();
        let b = Skeleton::from_mesh(&motion(&biped, &set, 0.75).unwrap()).unwrap();
        let foot = a.find("foot_l").unwrap();
        // Half a cycle apart the left foot is at opposite ends of its swing (relative to the hips).
        let rel = |s: &Skeleton| s.p[foot].z - s.p[0].z;
        assert!(rel(&a) * rel(&b) < 0.0, "{} {}", rel(&a), rel(&b));
        assert!(b.p[0].z > a.p[0].z, "the hips travel forward");
        let tail = skeleton(&SkeletonSettings { preset: 2, ..SkeletonSettings::defaults() });
        let wave = Skeleton::from_mesh(&motion(&tail, &MotionSettings { motion: 1, axis: 1, ..set.clone() }, 0.3).unwrap()).unwrap();
        assert!(wave.p.last().unwrap().x.abs() > 0.05);
        assert!(motion(&tail, &set, 0.3).is_err(), "walk needs biped names");
        // Blend 0 and 1 give the inputs; 0.5 is in between.
        let half = Skeleton::from_mesh(&blend(&motion(&biped, &set, 0.25).unwrap(), &motion(&biped, &set, 0.75).unwrap(), 0.5).unwrap()).unwrap();
        let zero = Skeleton::from_mesh(&blend(&motion(&biped, &set, 0.25).unwrap(), &motion(&biped, &set, 0.75).unwrap(), 0.0).unwrap()).unwrap();
        for j in 0..a.p.len() {
            assert!(close(zero.p[j], a.p[j]));
        }
        assert!(half.p[0].z > a.p[0].z && half.p[0].z < b.p[0].z);
    }

    #[test]
    fn capture_and_deform_follow_the_bones() {
        let chain = skeleton(&SkeletonSettings { preset: 1, joints: 3, length: 2.0, ..SkeletonSettings::defaults() });
        // Points along the chain, a little off to the side.
        let mesh = Mesh { vertices: (0..9).map(|i| Vec3::new(0.1, i as f32 * 0.25, 0.0)).collect(), ..Mesh::default() };
        let captured = capture(&mesh, &chain, &CaptureSettings::defaults()).unwrap();
        assert_eq!(captured.point_attributes[CAPTURE_WEIGHT].len(), 9 * MAX_INFLUENCES);
        for w in captured.point_attributes[CAPTURE_WEIGHT].chunks(MAX_INFLUENCES) {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        // Rest pose: nothing moves.
        let same = bone_deform(&captured, &chain, &chain).unwrap();
        for (a, b) in same.vertices.iter().zip(&mesh.vertices) {
            assert!(close(*a, *b));
        }
        // Bend the top bone: top points follow it, bottom points stay.
        let mut pose = RigPose::new();
        pose.joints[0] = "joint1".into();
        pose.values[0] = [0.0, 0.0, -90.0, 0.0, 0.0, 0.0];
        let bent = rig_pose(&chain, &pose).unwrap();
        let out = bone_deform(&captured, &chain, &bent).unwrap();
        // (Smooth weights give the bottom a sliver of the bent bone: well under a millimetre.)
        assert!(out.vertices[0].sub(mesh.vertices[0]).length() < 1e-3, "bottom moved: {:?}", out.vertices[0]);
        let top = out.vertices[8];
        assert!(top.x > 0.8 && top.y < 1.3, "top point at {top:?} should swing over");
        assert!(bone_deform(&mesh, &chain, &bent).is_err());
    }
}
