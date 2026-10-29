// Crowds: agents that walk, steer and avoid each other.
//
// The Crowd Solver takes start points (input 1, e.g. Scatter on a grid), an
// optional ground (input 2, agents walk on its surface) and optional obstacles
// (input 3). Each step every agent steers by:
// - separation from neighbours closer than the personal space,
// - alignment and cohesion with the group (flocking),
// - seeking the goal, slowing down on arrival,
// - wandering (a slowly drifting random direction),
// - turning away from obstacles ahead (their signed distance field).
// Turns are limited to a maximum rate, so agents curve instead of snapping.
//
// Output is points (N heading, up, orient, v, agent, speed, phase, pscale) for Copy to
// Points: orient turns a character facing +Z to face where its agent walks,
// or posed agents: simple mannequins whose legs and arms swing with a walk
// cycle timed to their speed.

use super::{hash01, Mesh, Vec3};
use crate::sims::{collider_field, run, step_count, SIM_FPS};
use std::collections::HashMap;
use std::sync::Arc;

pub const DISPLAYS: [&str; 2] = ["Points", "Agents"];

#[derive(Clone, Debug, PartialEq)]
pub struct CrowdSettings {
    pub speed: f32,
    pub speed_variance: f32,
    // Degrees per second.
    pub turn_rate: f32,
    pub personal_space: f32,
    pub separation: f32,
    pub alignment: f32,
    pub cohesion: f32,
    pub goal: [f32; 3],
    pub goal_weight: f32,
    pub wander: f32,
    pub avoid_distance: f32,
    pub agent_height: f32,
    pub display: usize,
    pub seed: u32,
}

impl CrowdSettings {
    pub fn defaults() -> Self {
        CrowdSettings {
            speed: 1.4,
            speed_variance: 0.2,
            turn_rate: 120.0,
            personal_space: 0.6,
            separation: 2.0,
            alignment: 0.3,
            cohesion: 0.1,
            goal: [0.0, 0.0, 0.0],
            goal_weight: 0.0,
            wander: 0.5,
            avoid_distance: 1.5,
            agent_height: 1.75,
            display: 1,
            seed: 1,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.speed,
            self.speed_variance,
            self.turn_rate,
            self.personal_space,
            self.separation,
            self.alignment,
            self.cohesion,
            self.goal[0],
            self.goal[1],
            self.goal[2],
            self.goal_weight,
            self.wander,
            self.avoid_distance,
            self.agent_height,
            self.display as f32,
            self.seed as f32,
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        CrowdSettings {
            speed: g(0, d.speed).max(0.0),
            speed_variance: g(1, d.speed_variance).clamp(0.0, 1.0),
            turn_rate: g(2, d.turn_rate).max(1.0),
            personal_space: g(3, d.personal_space).max(0.01),
            separation: g(4, d.separation).max(0.0),
            alignment: g(5, d.alignment).max(0.0),
            cohesion: g(6, d.cohesion).max(0.0),
            goal: [g(7, 0.0), g(8, 0.0), g(9, 0.0)],
            goal_weight: g(10, d.goal_weight).max(0.0),
            wander: g(11, d.wander).max(0.0),
            avoid_distance: g(12, d.avoid_distance).max(0.0),
            agent_height: g(13, d.agent_height).max(0.01),
            display: (g(14, d.display as f32).max(0.0) as usize).min(1),
            seed: g(15, d.seed as f32).max(0.0) as u32,
        }
    }
}

#[derive(Clone, Debug)]
struct Agent {
    p: Vec3,
    // Unit heading in the ground plane (y = 0).
    heading: Vec3,
    speed: f32,
    // This agent's cruising speed.
    cruise: f32,
    // Walk cycle phase in radians.
    phase: f32,
    wander: f32,
}

#[derive(Clone, Default)]
struct State {
    agents: Vec<Agent>,
}

// Ground heights: triangles bucketed on a grid in x/z, sampled straight down.
pub struct Ground {
    mesh: Mesh,
    cell: f32,
    buckets: HashMap<[i32; 2], Vec<usize>>,
}

impl Ground {
    pub fn new(mesh: Mesh) -> Option<Ground> {
        if mesh.indices.is_empty() {
            return None;
        }
        let (lo, hi) = mesh.bounds()?;
        let cell = ((hi.x - lo.x).max(hi.z - lo.z) / (mesh.indices.len() as f32).sqrt().max(1.0) * 2.0).max(1e-3);
        let mut buckets: HashMap<[i32; 2], Vec<usize>> = HashMap::new();
        for (t, tri) in mesh.indices.iter().enumerate() {
            let p = tri.map(|i| mesh.vertices[i]);
            let (x0, x1) = (p.iter().map(|v| v.x).fold(f32::MAX, f32::min), p.iter().map(|v| v.x).fold(f32::MIN, f32::max));
            let (z0, z1) = (p.iter().map(|v| v.z).fold(f32::MAX, f32::min), p.iter().map(|v| v.z).fold(f32::MIN, f32::max));
            for gz in (z0 / cell).floor() as i32..=(z1 / cell).floor() as i32 {
                for gx in (x0 / cell).floor() as i32..=(x1 / cell).floor() as i32 {
                    buckets.entry([gx, gz]).or_default().push(t);
                }
            }
        }
        Some(Ground { mesh, cell, buckets })
    }

    // The highest ground surface at (x, z), if any.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let key = [(x / self.cell).floor() as i32, (z / self.cell).floor() as i32];
        let mut best: Option<f32> = None;
        for &t in self.buckets.get(&key)? {
            let [a, b, c] = self.mesh.indices[t].map(|i| self.mesh.vertices[i]);
            let det = (b.z - c.z) * (a.x - c.x) + (c.x - b.x) * (a.z - c.z);
            if det.abs() < 1e-12 {
                continue;
            }
            let l1 = ((b.z - c.z) * (x - c.x) + (c.x - b.x) * (z - c.z)) / det;
            let l2 = ((c.z - a.z) * (x - c.x) + (a.x - c.x) * (z - c.z)) / det;
            let l3 = 1.0 - l1 - l2;
            if l1 >= -1e-5 && l2 >= -1e-5 && l3 >= -1e-5 {
                let y = l1 * a.y + l2 * b.y + l3 * c.y;
                best = Some(best.map_or(y, |b: f32| b.max(y)));
            }
        }
        best
    }
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

// Rotates unit `from` towards `to` (both in the ground plane) by at most `max` radians.
fn turn_towards(from: Vec3, to: Vec3, max: f32) -> Vec3 {
    let to = flat(to).normalize();
    if to.length() < 0.5 {
        return from;
    }
    let angle = from.z.atan2(from.x);
    let target = to.z.atan2(to.x);
    let mut delta = target - angle;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    let a = angle + delta.clamp(-max, max);
    Vec3::new(a.cos(), 0.0, a.sin())
}

fn init(points: &Mesh, set: &CrowdSettings, ground: Option<&Ground>) -> State {
    let agents = points
        .vertices
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let h = |k: u32| hash01(i as u32 * 13 + k, set.seed);
            let angle = h(1) * std::f32::consts::TAU;
            let mut p = *p;
            if let Some(y) = ground.and_then(|g| g.height(p.x, p.z)) {
                p.y = y;
            }
            let cruise = set.speed * (1.0 + set.speed_variance * (h(2) * 2.0 - 1.0));
            Agent { p, heading: Vec3::new(angle.cos(), 0.0, angle.sin()), speed: 0.0, cruise, phase: h(3) * std::f32::consts::TAU, wander: angle }
        })
        .collect();
    State { agents }
}

fn step(state: &mut State, set: &CrowdSettings, dt: f32, ground: Option<&Ground>, obstacles: Option<&crate::volume::Volume>, at: usize) {
    let space = set.personal_space;
    let radius = (space * 3.0).max(1e-3);
    let mut cells: HashMap<[i32; 2], Vec<usize>> = HashMap::new();
    for (i, a) in state.agents.iter().enumerate() {
        cells.entry([(a.p.x / radius).floor() as i32, (a.p.z / radius).floor() as i32]).or_default().push(i);
    }
    let goal = Vec3::from_arr(set.goal);
    let previous = state.agents.clone();
    for (i, agent) in state.agents.iter_mut().enumerate() {
        let (mut separation, mut align, mut centre, mut neighbours) = (Vec3::zero(), Vec3::zero(), Vec3::zero(), 0usize);
        let c = [(agent.p.x / radius).floor() as i32, (agent.p.z / radius).floor() as i32];
        for dz in -1..=1 {
            for dx in -1..=1 {
                for &j in cells.get(&[c[0] + dx, c[1] + dz]).into_iter().flatten() {
                    if j == i {
                        continue;
                    }
                    let other = &previous[j];
                    let d = flat(agent.p.sub(other.p));
                    let dist = d.length();
                    if dist > radius {
                        continue;
                    }
                    if dist < space * 2.0 && dist > 1e-6 {
                        // Stronger the closer they get.
                        separation = separation.add(d.scale(1.0 / dist * (space * 2.0 - dist) / space));
                    }
                    align = align.add(other.heading);
                    centre = centre.add(other.p);
                    neighbours += 1;
                }
            }
        }
        // Wander: a heading that drifts with noise over time.
        agent.wander += (hash01(i as u32 * 31 + at as u32, set.seed ^ 0x51) - 0.5) * 2.0 * dt * 3.0;
        let mut desired = agent.heading.add(Vec3::new(agent.wander.cos(), 0.0, agent.wander.sin()).scale(set.wander));
        desired = desired.add(separation.scale(set.separation));
        if neighbours > 0 {
            desired = desired.add(align.scale(set.alignment / neighbours as f32));
            desired = desired.add(flat(centre.scale(1.0 / neighbours as f32).sub(agent.p)).normalize().scale(set.cohesion));
        }
        let mut target_speed = agent.cruise;
        if set.goal_weight > 0.0 {
            let to_goal = flat(goal.sub(agent.p));
            let dist = to_goal.length();
            desired = desired.add(to_goal.normalize().scale(set.goal_weight));
            // Slow down over the last couple of metres; stop at the goal.
            target_speed *= (dist / 2.0).clamp(0.0, 1.0);
        }
        // Obstacles: look ahead and steer down the distance field's gradient.
        if let Some(sdf) = obstacles {
            let ahead = agent.p.add(agent.heading.scale(set.avoid_distance * 0.5)).add(Vec3::new(0.0, set.agent_height * 0.5, 0.0));
            let d = sdf.sample(ahead);
            if d < set.avoid_distance {
                // Follow the obstacle's surface on the side the agent wants to go, pushed off it a little.
                let away = flat(sdf.gradient(ahead)).normalize();
                let mut along = Vec3::new(-away.z, 0.0, away.x);
                if along.dot(desired) < 0.0 {
                    along = along.scale(-1.0);
                }
                let strength = (1.0 - d / set.avoid_distance.max(1e-3)).clamp(0.0, 1.0) * 4.0;
                let into = desired.dot(away).min(0.0);
                desired = desired.sub(away.scale(into * strength.min(1.0))).add(along.scale(strength)).add(away.scale(strength * 0.3));
            }
        }
        agent.heading = turn_towards(agent.heading, desired, set.turn_rate.to_radians() * dt);
        agent.speed += (target_speed - agent.speed) * (dt * 3.0).min(1.0);
        let mut p = agent.p.add(agent.heading.scale(agent.speed * dt));
        if let Some(sdf) = obstacles {
            // Never walk into an obstacle.
            let probe = p.add(Vec3::new(0.0, set.agent_height * 0.5, 0.0));
            let d = sdf.sample(probe) - space * 0.5;
            if d < 0.0 {
                p = p.sub(flat(sdf.gradient(probe)).normalize().scale(d));
            }
        }
        if let Some(y) = ground.and_then(|g| g.height(p.x, p.z)) {
            p.y = y;
        }
        agent.p = p;
        // About 0.75 m per stride (scaled with height): faster walkers step faster.
        let stride = 0.75 * set.agent_height / 1.75;
        agent.phase = (agent.phase + agent.speed * dt / stride * std::f32::consts::PI) % std::f32::consts::TAU;
    }
}

// A box from `a` to `b` (a limb segment) with half widths `w`, given the agent's frame.
fn limb(mesh: &mut Mesh, a: Vec3, b: Vec3, w: f32, side: Vec3) {
    let axis = b.sub(a);
    let len = axis.length().max(1e-5);
    let dir = axis.scale(1.0 / len);
    let u = side.sub(dir.scale(side.dot(dir))).normalize();
    let v = dir.cross(u);
    let base = mesh.vertices.len();
    for k in 0..8 {
        let (s, t, end) = (if k & 1 == 0 { -w } else { w }, if k & 2 == 0 { -w } else { w }, if k & 4 == 0 { a } else { b });
        mesh.vertices.push(end.add(u.scale(s)).add(v.scale(t)));
    }
    for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
        mesh.indices.push([base + f[0], base + f[2], base + f[1]]);
        mesh.indices.push([base + f[0], base + f[3], base + f[2]]);
    }
}

// A mannequin posed by the walk cycle: torso, head, arms and legs as boxes.
fn mannequin(mesh: &mut Mesh, agent: &Agent, height: f32, cruise: f32) {
    let s = height / 1.75;
    let fwd = agent.heading;
    let up = Vec3::new(0.0, 1.0, 0.0);
    let side = up.cross(fwd).normalize();
    let at = |x: f32, y: f32, z: f32| agent.p.add(side.scale(x * s)).add(up.scale(y * s)).add(fwd.scale(z * s));
    // Swing grows with speed; idle agents stand.
    let swing = 0.5 * (agent.speed / cruise.max(0.1)).clamp(0.0, 1.0);
    let bob = 0.02 * (2.0 * agent.phase).cos().abs() * swing * 2.0;
    let leg = |sign: f32, mesh: &mut Mesh| {
        let a = (agent.phase + if sign > 0.0 { 0.0 } else { std::f32::consts::PI }).sin() * swing;
        let hip = at(0.1 * sign, 0.95 + bob, 0.0);
        let knee_bend = (agent.phase + if sign > 0.0 { 0.0 } else { std::f32::consts::PI } + 1.2).sin().max(0.0) * swing;
        let knee = hip.add(fwd.scale(a.sin() * 0.45 * s)).sub(up.scale(a.cos() * 0.45 * s));
        let ankle = knee.add(fwd.scale((a - knee_bend).sin() * 0.45 * s)).sub(up.scale((a - knee_bend).cos() * 0.45 * s));
        limb(mesh, hip, knee, 0.065 * s, side);
        limb(mesh, knee, ankle, 0.055 * s, side);
        limb(mesh, ankle, ankle.add(fwd.scale(0.18 * s)), 0.045 * s, side);
    };
    let arm = |sign: f32, mesh: &mut Mesh| {
        // Arms swing opposite to the leg on the same side.
        let a = -(agent.phase + if sign > 0.0 { 0.0 } else { std::f32::consts::PI }).sin() * swing * 0.8;
        let shoulder = at(0.21 * sign, 1.45 + bob, 0.0);
        let elbow = shoulder.add(fwd.scale(a.sin() * 0.3 * s)).sub(up.scale(a.cos() * 0.3 * s));
        let wrist = elbow.add(fwd.scale((a + 0.3 * swing).sin() * 0.28 * s)).sub(up.scale((a + 0.3 * swing).cos() * 0.28 * s));
        limb(mesh, shoulder, elbow, 0.045 * s, side);
        limb(mesh, elbow, wrist, 0.04 * s, side);
    };
    leg(1.0, mesh);
    leg(-1.0, mesh);
    limb(mesh, at(0.0, 0.92 + bob, 0.0), at(0.0, 1.5 + bob, 0.0), 0.16 * s, side);
    limb(mesh, at(0.0, 1.5 + bob, 0.0), at(0.0, 1.56 + bob, 0.0), 0.05 * s, side);
    limb(mesh, at(0.0, 1.56 + bob, 0.0), at(0.0, 1.75 + bob, 0.0), 0.1 * s, side);
    arm(1.0, mesh);
    arm(-1.0, mesh);
}

fn output(state: &State, set: &CrowdSettings) -> Mesh {
    if set.display == 1 {
        let mut mesh = Mesh::default();
        let (mut agent_prim, mut colours) = (Vec::new(), Vec::new());
        for (i, a) in state.agents.iter().enumerate() {
            let before = mesh.indices.len();
            mannequin(&mut mesh, a, set.agent_height, a.cruise);
            // A shirt colour per agent.
            let h = |k: u32| hash01(i as u32 * 17 + k, set.seed ^ 0xc0);
            let colour = [0.2 + 0.7 * h(1), 0.2 + 0.6 * h(2), 0.2 + 0.7 * h(3)];
            for _ in before..mesh.indices.len() {
                agent_prim.push(i as f32);
                colours.extend(colour);
            }
        }
        mesh.point_attributes.insert("prim:agent".into(), agent_prim);
        mesh.point_attributes.insert("prim:Cd".into(), colours);
        return mesh;
    }
    let mut mesh = Mesh { vertices: state.agents.iter().map(|a| a.p).collect(), ..Mesh::default() };
    let vec_attr = |f: &dyn Fn(&Agent) -> Vec3| state.agents.iter().flat_map(|a| {
        let v = f(a);
        [v.x, v.y, v.z]
    }).collect::<Vec<f32>>();
    mesh.point_attributes.insert("N".into(), vec_attr(&|a| a.heading));
    mesh.point_attributes.insert("up".into(), vec_attr(&|_| Vec3::new(0.0, 1.0, 0.0)));
    // A turn about Y from +Z to the heading, as a quaternion (x y z w): Copy to Points turns
    // copies by it, so a character facing +Z faces where its agent walks.
    let orient: Vec<f32> = state
        .agents
        .iter()
        .flat_map(|a| {
            let half = a.heading.x.atan2(a.heading.z) * 0.5;
            [0.0, half.sin(), 0.0, half.cos()]
        })
        .collect();
    mesh.point_attributes.insert("orient".into(), orient);
    mesh.point_attributes.insert("v".into(), vec_attr(&|a| a.heading.scale(a.speed)));
    mesh.point_attributes.insert("agent".into(), (0..state.agents.len()).map(|i| i as f32).collect());
    mesh.point_attributes.insert("speed".into(), state.agents.iter().map(|a| a.speed).collect());
    mesh.point_attributes.insert("phase".into(), state.agents.iter().map(|a| a.phase).collect());
    mesh.point_attributes.insert("pscale".into(), vec![set.agent_height / 1.75; state.agents.len()]);
    mesh
}

pub fn crowd(
    key: u64,
    set: &CrowdSettings,
    time: f32,
    start: impl FnOnce() -> Mesh,
    ground: impl FnOnce() -> Mesh,
    obstacles: impl FnOnce() -> Mesh,
) -> Mesh {
    let dt = 1.0 / SIM_FPS;
    let ground = Ground::new(ground());
    let mut obstacles = Some(obstacles);
    let mut field: Option<Option<Arc<crate::volume::Volume>>> = None;
    run(
        key,
        step_count(time, 1),
        12,
        || init(&start(), set, ground.as_ref()),
        Clone::clone,
        Clone::clone,
        |state, at| {
            let field = field.get_or_insert_with(|| obstacles.take().and_then(|o| collider_field(&o())));
            step(state, set, dt, ground.as_ref(), field.as_deref(), at);
        },
        |state| output(state, set),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(n: usize, spacing: f32) -> Mesh {
        let side = (n as f32).sqrt().ceil() as usize;
        Mesh { vertices: (0..n).map(|i| Vec3::new((i % side) as f32 * spacing, 0.0, (i / side) as f32 * spacing)).collect(), ..Mesh::default() }
    }

    fn slope() -> Mesh {
        // A 40 x 40 ramp rising 0.25 per unit in x.
        let mut mesh = Mesh::default();
        for z in 0..2 {
            for x in 0..2 {
                let (px, pz) = (x as f32 * 40.0 - 20.0, z as f32 * 40.0 - 20.0);
                mesh.vertices.push(Vec3::new(px, px * 0.25, pz));
            }
        }
        mesh.indices = vec![[0, 2, 1], [1, 2, 3]];
        mesh
    }

    #[test]
    fn agents_seek_the_goal_without_colliding() {
        crate::sims::clear_all();
        let set = CrowdSettings { goal: [10.0, 0.0, 10.0], goal_weight: 2.0, display: 0, ..CrowdSettings::defaults() };
        let start = starts(25, 0.8);
        let dist = |m: &Mesh| m.vertices.iter().map(|p| flat(p.sub(Vec3::from_arr(set.goal))).length()).sum::<f32>() / m.vertices.len() as f32;
        let before = dist(&start);
        let out = crowd(81, &set, 8.0, || start.clone(), Mesh::default, Mesh::default);
        assert_eq!(out.vertices.len(), 25);
        assert!(dist(&out) < before * 0.5, "{} -> {}", before, dist(&out));
        // Personal space keeps them apart even when they bunch up at the goal.
        let mut closest = f32::MAX;
        for i in 0..25 {
            for j in i + 1..25 {
                closest = closest.min(flat(out.vertices[i].sub(out.vertices[j])).length());
            }
        }
        assert!(closest > 0.2, "agents {closest} apart");
        assert_eq!(out.point_attributes["N"].len(), 75);
        // orient turns +Z onto each agent's heading.
        let (n, q) = (&out.point_attributes["N"], &out.point_attributes["orient"]);
        for i in 0..25 {
            let heading = Vec3::new(n[i * 3], 0.0, n[i * 3 + 2]).normalize();
            let m = crate::vexmath::q_to_m3(&[q[i * 4], q[i * 4 + 1], q[i * 4 + 2], q[i * 4 + 3]]);
            let rows = [Vec3::new(m[0], m[1], m[2]), Vec3::new(m[3], m[4], m[5]), Vec3::new(m[6], m[7], m[8])];
            let forward = crate::kinefx::vm(Vec3::new(0.0, 0.0, 1.0), &rows);
            assert!(forward.sub(heading).length() < 1e-4, "agent {i}: {forward:?} vs {heading:?}");
        }
    }

    #[test]
    fn agents_walk_on_the_ground_and_animate() {
        crate::sims::clear_all();
        let set = CrowdSettings { display: 0, ..CrowdSettings::defaults() };
        let out = crowd(82, &set, 3.0, || starts(9, 1.5), slope, Mesh::default);
        for p in &out.vertices {
            assert!((p.y - p.x * 0.25).abs() < 1e-3, "{p:?} is off the ground");
        }
        crate::sims::clear_all();
        let agents = crowd(83, &CrowdSettings::defaults(), 1.0, || starts(4, 2.0), Mesh::default, Mesh::default);
        assert_eq!(agents.indices.len(), 4 * 13 * 12, "13 boxes per mannequin");
        assert_eq!(agents.point_attributes["prim:agent"].len(), agents.indices.len());
        // Legs move between frames.
        let later = crowd(83, &CrowdSettings::defaults(), 1.25, || starts(4, 2.0), Mesh::default, Mesh::default);
        assert_ne!(agents.vertices, later.vertices);
        let round = CrowdSettings::from_params(&set.to_params());
        assert_eq!(round, set);
    }

    #[test]
    fn agents_go_around_obstacles() {
        crate::sims::clear_all();
        // A wall across the path to the goal.
        let mut wall = Mesh::default();
        let (lo, hi) = (Vec3::new(4.0, 0.0, -3.0), Vec3::new(4.6, 2.0, 3.0));
        for k in 0..8 {
            wall.vertices.push(Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z }));
        }
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            wall.indices.push([f[0], f[1], f[2]]);
            wall.indices.push([f[0], f[2], f[3]]);
        }
        let set = CrowdSettings { goal: [9.0, 0.0, 0.0], goal_weight: 2.0, wander: 0.0, display: 0, ..CrowdSettings::defaults() };
        let start = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.2)], ..Mesh::default() };
        let mut crossed_inside = false;
        for frame in 1..=240 {
            let out = crowd(84, &set, frame as f32 / 24.0, || start.clone(), Mesh::default, || wall.clone());
            let p = out.vertices[0];
            if p.x > lo.x && p.x < hi.x && p.z > lo.z && p.z < hi.z {
                crossed_inside = true;
            }
        }
        let end = crowd(84, &set, 10.0, || start.clone(), Mesh::default, || wall.clone()).vertices[0];
        assert!(!crossed_inside, "walked through the wall");
        assert!(end.x > hi.x, "got stuck behind the wall at {end:?}");
    }
}
