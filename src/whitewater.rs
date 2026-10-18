// Whitewater: spray, foam and bubbles from a FLIP simulation.
//
// After Ihmsen et al., "Unified Spray, Foam and Bubbles for Particle-Based
// Fluids" (2012). Each frame the fluid particles are scored for trapped air
// (neighbours moving apart or colliding), wave crests (convex surface moving
// outwards) and kinetic energy; high scores emit whitewater particles. Those
// are classified by how many fluid particles surround them:
// - spray (few): ballistic, with gravity and air drag;
// - foam (surface): carried by the fluid, fading over its lifespan;
// - bubbles (deep): pulled along by the fluid and pushed up by buoyancy.

use super::{hash01, Mesh, Vec3};
use crate::sims::{collider_field, run, step_count, SIM_FPS};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct WhitewaterSettings {
    pub max_particles: usize,
    // Whitewater particles per second from a fully scoring fluid particle.
    pub emission_rate: f32,
    pub trapped_air: f32,
    pub wave_crest: f32,
    // Fluid speeds (world units per second) mapped to 0 and full emission.
    pub min_speed: f32,
    pub max_speed: f32,
    // Seconds a foam particle lasts.
    pub lifespan: f32,
    pub spray_drag: f32,
    pub bubble_buoyancy: f32,
    pub bubble_drag: f32,
    // Fluid neighbours below which a particle is spray, above which a bubble.
    pub spray_neighbours: usize,
    pub bubble_neighbours: usize,
    pub gravity: f32,
    pub particle_size: f32,
    pub substeps: usize,
    pub seed: u32,
}

impl WhitewaterSettings {
    pub fn defaults() -> Self {
        WhitewaterSettings {
            max_particles: 200_000,
            emission_rate: 400.0,
            trapped_air: 1.0,
            wave_crest: 1.0,
            min_speed: 1.0,
            max_speed: 6.0,
            lifespan: 2.0,
            spray_drag: 0.3,
            bubble_buoyancy: 2.0,
            bubble_drag: 0.5,
            spray_neighbours: 6,
            bubble_neighbours: 20,
            gravity: -9.8,
            particle_size: 0.01,
            substeps: 1,
            seed: 1,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.max_particles as f32,
            self.emission_rate,
            self.trapped_air,
            self.wave_crest,
            self.min_speed,
            self.max_speed,
            self.lifespan,
            self.spray_drag,
            self.bubble_buoyancy,
            self.bubble_drag,
            self.spray_neighbours as f32,
            self.bubble_neighbours as f32,
            self.gravity,
            self.particle_size,
            self.substeps as f32,
            self.seed as f32,
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        WhitewaterSettings {
            max_particles: g(0, d.max_particles as f32).clamp(0.0, 5e6) as usize,
            emission_rate: g(1, d.emission_rate).max(0.0),
            trapped_air: g(2, d.trapped_air).max(0.0),
            wave_crest: g(3, d.wave_crest).max(0.0),
            min_speed: g(4, d.min_speed).max(0.0),
            max_speed: g(5, d.max_speed).max(0.01),
            lifespan: g(6, d.lifespan).max(0.01),
            spray_drag: g(7, d.spray_drag).max(0.0),
            bubble_buoyancy: g(8, d.bubble_buoyancy),
            bubble_drag: g(9, d.bubble_drag).clamp(0.0, 1.0),
            spray_neighbours: g(10, d.spray_neighbours as f32).max(0.0) as usize,
            bubble_neighbours: g(11, d.bubble_neighbours as f32).max(1.0) as usize,
            gravity: g(12, d.gravity),
            particle_size: g(13, d.particle_size).max(1e-5),
            substeps: g(14, d.substeps as f32).clamp(1.0, 8.0) as usize,
            seed: g(15, d.seed as f32).max(0.0) as u32,
        }
    }
}

pub const SPRAY: u8 = 0;
pub const FOAM: u8 = 1;
pub const BUBBLE: u8 = 2;
pub const TYPES: [&str; 3] = ["spray", "foam", "bubble"];

#[derive(Clone, Debug)]
struct Particle {
    p: Vec3,
    v: Vec3,
    // Seconds of foam life left.
    life: f32,
    age: f32,
    kind: u8,
}

#[derive(Clone, Default)]
struct State {
    particles: Vec<Particle>,
    // Emitted so far, for random numbers that don't repeat.
    born: u32,
}

// Fluid particles in a hash grid with cells one kernel radius wide.
struct Fluid {
    points: Vec<Vec3>,
    velocity: Vec<Vec3>,
    radius: f32,
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl Fluid {
    fn new(mesh: &Mesh) -> Option<Fluid> {
        let n = mesh.vertices.len();
        if n < 8 {
            return None;
        }
        let v = mesh.point_attributes.get("v").filter(|v| v.len() == n * 3);
        let velocity = (0..n).map(|i| v.map_or(Vec3::zero(), |v| Vec3::new(v[i * 3], v[i * 3 + 1], v[i * 3 + 2]))).collect();
        // Particle spacing from the bounding volume, then a kernel of two spacings.
        let (lo, hi) = mesh.bounds()?;
        let e = hi.sub(lo);
        let volume = (e.x.max(1e-3) * e.y.max(1e-3) * e.z.max(1e-3)).max(1e-9);
        let spacing = (volume / n as f32).cbrt();
        let radius = (spacing * 2.0).max(1e-4);
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (i, p) in mesh.vertices.iter().enumerate() {
            cells.entry(Self::cell(*p, radius)).or_default().push(i as u32);
        }
        Some(Fluid { points: mesh.vertices.clone(), velocity, radius, cells })
    }

    fn cell(p: Vec3, r: f32) -> [i32; 3] {
        [(p.x / r).floor() as i32, (p.y / r).floor() as i32, (p.z / r).floor() as i32]
    }

    // Calls `f(index, distance)` for fluid particles within the kernel radius of `p`.
    fn neighbours(&self, p: Vec3, mut f: impl FnMut(usize, f32)) {
        let c = Self::cell(p, self.radius);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &j in list {
                            let d = self.points[j as usize].sub(p).length();
                            if d < self.radius {
                                f(j as usize, d);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)
}

// Whitewater emitted around one fluid particle this step: (position, velocity).
fn emit(state: &mut State, fluid: &Fluid, set: &WhitewaterSettings, dt: f32) {
    let n = fluid.points.len();
    let r = fluid.radius;
    // Pass 1: surface normals (pointing out of the fluid) and neighbour counts.
    let mut normals = vec![Vec3::zero(); n];
    let mut counts = vec![0usize; n];
    for i in 0..n {
        let pi = fluid.points[i];
        let mut grad = Vec3::zero();
        fluid.neighbours(pi, |j, d| {
            counts[i] += 1;
            if j != i && d > 1e-9 {
                grad = grad.add(pi.sub(fluid.points[j]).scale((1.0 - d / r) / d));
            }
        });
        normals[i] = grad.normalize();
    }
    let mean = counts.iter().sum::<usize>() as f32 / n as f32;
    let room = set.max_particles.saturating_sub(state.particles.len());
    let mut budget = room;
    for i in 0..n {
        if budget == 0 {
            break;
        }
        let (pi, vi) = (fluid.points[i], fluid.velocity[i]);
        let speed = vi.length();
        let energy = ramp(speed, set.min_speed, set.max_speed);
        if energy <= 0.0 {
            continue;
        }
        let (mut trapped, mut crest, mut weight) = (0.0f32, 0.0f32, 0.0f32);
        let surface = (counts[i] as f32) < 0.75 * mean;
        let ni = normals[i];
        fluid.neighbours(pi, |j, d| {
            if j == i || d < 1e-9 {
                return;
            }
            let w = 1.0 - d / r;
            weight += w;
            let vij = vi.sub(fluid.velocity[j]);
            let xij = pi.sub(fluid.points[j]).scale(1.0 / d);
            let rel = vij.length();
            if rel > 1e-6 {
                trapped += rel * (1.0 - vij.scale(1.0 / rel).dot(xij)) * w;
            }
            if surface && xij.dot(ni) < 0.0 {
                crest += (1.0 - ni.dot(normals[j])) * w;
            }
        });
        if weight <= 0.0 {
            continue;
        }
        let trapped = ramp(trapped / weight, 0.5, 4.0);
        // Only crests moving outwards break into spray.
        let crest = if surface && vi.scale(1.0 / speed).dot(ni) > 0.6 { ramp(crest / weight, 0.1, 0.6) } else { 0.0 };
        let rate = set.emission_rate * (set.trapped_air * trapped + set.wave_crest * crest) * energy;
        let key = state.born.wrapping_mul(7919).wrapping_add(i as u32);
        let count = (rate * dt + hash01(key, set.seed ^ 0xa11ce)) as usize;
        if count == 0 {
            continue;
        }
        // A disc perpendicular to the velocity, swept along this step's motion.
        let dir = vi.scale(1.0 / speed);
        let helper = if dir.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
        let e1 = dir.cross(helper).normalize();
        let e2 = dir.cross(e1);
        for k in 0..count.min(budget) {
            let h = |s: u32| hash01(state.born.wrapping_mul(3).wrapping_add(s), set.seed.wrapping_add(k as u32));
            let (radius, angle, along) = (0.5 * r * h(1).sqrt(), h(2) * std::f32::consts::TAU, h(3) * speed * dt);
            let side = e1.scale(angle.cos()).add(e2.scale(angle.sin()));
            state.particles.push(Particle {
                p: pi.add(side.scale(radius)).add(dir.scale(along)),
                v: vi.add(side.scale(radius / dt.max(1e-4) * 0.05)),
                life: set.lifespan * (0.5 + h(4)),
                age: 0.0,
                kind: SPRAY,
            });
            state.born = state.born.wrapping_add(1);
            budget -= 1;
        }
    }
}

// Classifies and moves the whitewater particles.
fn advect(state: &mut State, fluid: Option<&Fluid>, collider: Option<&crate::volume::Volume>, set: &WhitewaterSettings, dt: f32) {
    let gravity = Vec3::new(0.0, set.gravity, 0.0);
    for q in &mut state.particles {
        let (mut count, mut vel, mut wsum) = (0usize, Vec3::zero(), 0.0f32);
        if let Some(fluid) = fluid {
            fluid.neighbours(q.p, |j, d| {
                count += 1;
                let w = 1.0 - d / fluid.radius;
                vel = vel.add(fluid.velocity[j].scale(w));
                wsum += w;
            });
        }
        let fluid_v = if wsum > 0.0 { vel.scale(1.0 / wsum) } else { Vec3::zero() };
        q.kind = if count < set.spray_neighbours {
            SPRAY
        } else if count > set.bubble_neighbours {
            BUBBLE
        } else {
            FOAM
        };
        match q.kind {
            SPRAY => {
                q.v = q.v.add(gravity.scale(dt));
                q.v = q.v.scale((1.0 - set.spray_drag * dt).max(0.0));
            }
            FOAM => {
                q.v = fluid_v;
                q.life -= dt;
            }
            _ => {
                q.v = q.v.add(gravity.scale(-set.bubble_buoyancy * dt)).add(fluid_v.sub(q.v).scale(set.bubble_drag));
            }
        }
        q.p = q.p.add(q.v.scale(dt));
        q.age += dt;
        if let Some(sdf) = collider {
            let d = sdf.sample(q.p);
            if d < 0.0 {
                // Pushed back to the surface; spray loses its inward speed and some friction.
                let n = sdf.gradient(q.p).normalize();
                q.p = q.p.sub(n.scale(d));
                let vn = q.v.dot(n);
                if vn < 0.0 {
                    q.v = q.v.sub(n.scale(vn)).scale(0.7);
                }
            }
        }
    }
    // Foam dies when its life runs out; anything very old or far below goes too.
    let floor = fluid.and_then(|f| f.points.iter().map(|p| p.y).reduce(f32::min)).unwrap_or(f32::NEG_INFINITY) - 10.0;
    let max_age = set.lifespan * 20.0;
    state.particles.retain(|q| q.life > 0.0 && q.age < max_age && q.p.y > floor && q.p.x.is_finite());
}

fn output(state: &State, set: &WhitewaterSettings) -> Mesh {
    let n = state.particles.len();
    let mut mesh = Mesh { vertices: state.particles.iter().map(|q| q.p).collect(), ..Mesh::default() };
    let attr = |f: &dyn Fn(&Particle) -> f32| state.particles.iter().map(f).collect::<Vec<f32>>();
    mesh.point_attributes.insert("v".into(), state.particles.iter().flat_map(|q| [q.v.x, q.v.y, q.v.z]).collect());
    mesh.point_attributes.insert("age".into(), attr(&|q| q.age));
    mesh.point_attributes.insert("life".into(), attr(&|q| (q.life / set.lifespan).clamp(0.0, 1.0)));
    mesh.point_attributes.insert("type".into(), attr(&|q| q.kind as f32));
    for (kind, name) in TYPES.iter().enumerate() {
        mesh.point_attributes.insert(format!("group:{name}"), attr(&|q| (q.kind as usize == kind) as u8 as f32));
    }
    // Spray droplets are small; foam shrinks as it fades.
    mesh.point_attributes.insert(
        "pscale".into(),
        attr(&|q| set.particle_size * match q.kind {
            SPRAY => 0.6,
            FOAM => 0.4 + 0.6 * (q.life / set.lifespan).clamp(0.0, 1.0),
            _ => 0.5,
        }),
    );
    mesh.point_attributes.insert(
        "Cd".into(),
        state.particles.iter().flat_map(|q| if q.kind == BUBBLE { [0.75, 0.85, 0.95] } else { [0.95, 0.97, 1.0] }).collect(),
    );
    // The renderer draws these points as spheres of radius pscale.
    mesh.point_attributes.insert(RENDER_POINTS.into(), vec![1.0]);
    let _ = n;
    mesh
}

// Detail attribute marking points that render as spheres.
pub const RENDER_POINTS: &str = "detail:render_points";

pub fn whitewater(
    key: u64,
    set: &WhitewaterSettings,
    time: f32,
    mut fluid_at: impl FnMut(f32) -> Mesh,
    collider: impl FnOnce() -> Mesh,
) -> Mesh {
    let substeps = set.substeps.clamp(1, 8);
    let dt = 1.0 / (SIM_FPS * substeps as f32);
    let mut collider = Some(collider);
    let mut field: Option<Option<Arc<crate::volume::Volume>>> = None;
    // The fluid is read once per frame (the FLIP solver steps per frame too).
    let mut frame_fluid: Option<(usize, Option<Fluid>)> = None;
    run(
        key,
        step_count(time, substeps),
        substeps * 12,
        State::default,
        Clone::clone,
        Clone::clone,
        |state, at| {
            let field = field.get_or_insert_with(|| collider.take().and_then(|c| collider_field(&c())));
            let frame = at.div_ceil(substeps);
            if frame_fluid.as_ref().is_none_or(|(f, _)| *f != frame) {
                frame_fluid = Some((frame, Fluid::new(&fluid_at(frame as f32 / SIM_FPS))));
            }
            let fluid = frame_fluid.as_ref().and_then(|(_, f)| f.as_ref());
            if let Some(fluid) = fluid {
                emit(state, fluid, set, dt);
            }
            advect(state, fluid, field.as_deref(), set, dt);
        },
        |state| output(state, set),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // A block of fluid particles on a grid, all moving with `velocity`, plus a
    // fast jet column rising out of its top.
    fn splash(jet: f32) -> Mesh {
        let mut mesh = Mesh::default();
        let mut v = Vec::new();
        let s = 0.05;
        for z in 0..20 {
            for y in 0..10 {
                for x in 0..20 {
                    mesh.vertices.push(Vec3::new(x as f32 * s, y as f32 * s, z as f32 * s));
                    // The middle columns shoot upwards and outwards; the rest is still.
                    let centre = (x as f32 - 9.5).abs() < 3.0 && (z as f32 - 9.5).abs() < 3.0;
                    let up = if centre && y > 5 { jet } else { 0.0 };
                    v.extend([(x as f32 - 9.5) * up * 0.1, up, (z as f32 - 9.5) * up * 0.1]);
                }
            }
        }
        mesh.point_attributes.insert("v".into(), v);
        mesh
    }

    #[test]
    fn calm_fluid_makes_no_whitewater() {
        crate::sims::clear_all();
        let still = splash(0.0);
        let out = whitewater(71, &WhitewaterSettings::defaults(), 1.0, |_| still.clone(), Mesh::default);
        assert!(out.vertices.is_empty());
    }

    #[test]
    fn splashes_make_spray_foam_and_bubbles() {
        crate::sims::clear_all();
        let fluid = splash(5.0);
        let set = WhitewaterSettings { emission_rate: 2000.0, ..WhitewaterSettings::defaults() };
        let out = whitewater(72, &set, 0.5, |_| fluid.clone(), Mesh::default);
        assert!(out.vertices.len() > 100, "{} particles", out.vertices.len());
        assert!(out.vertices.len() <= set.max_particles);
        let types = &out.point_attributes["type"];
        assert_eq!(types.len(), out.vertices.len());
        // Spray has left the fluid; some is still among it as foam or bubbles.
        assert!(types.iter().any(|t| *t == SPRAY as f32));
        assert!(types.iter().any(|t| *t != SPRAY as f32));
        let spray_high = out.vertices.iter().zip(types).filter(|(_, t)| **t == SPRAY as f32).any(|(p, _)| p.y > 0.6);
        assert!(spray_high, "spray should fly above the fluid");
        for name in TYPES {
            assert_eq!(out.point_attributes[&format!("group:{name}")].len(), out.vertices.len());
        }
        assert_eq!(out.point_attributes[RENDER_POINTS], vec![1.0]);
        // Stepping on from the cache matches a fresh run.
        let later = whitewater(72, &set, 0.75, |_| fluid.clone(), Mesh::default);
        crate::sims::clear_all();
        let fresh = whitewater(72, &set, 0.75, |_| fluid.clone(), Mesh::default);
        assert_eq!(later.vertices, fresh.vertices);
        // A cap keeps the count down.
        crate::sims::clear_all();
        let capped = whitewater(73, &WhitewaterSettings { max_particles: 50, ..set.clone() }, 0.5, |_| fluid.clone(), Mesh::default);
        assert!(capped.vertices.len() <= 50);
        let round = WhitewaterSettings::from_params(&set.to_params());
        assert_eq!(round, set);
    }
}
