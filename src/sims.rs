// Stateful solvers. Unlike the closed-form particle nodes, these keep their
// simulation state between cooks: a shared cache holds each solver's latest
// state plus periodic checkpoints, so playing forward only computes the new
// steps and scrubbing back resumes from the nearest earlier checkpoint.
//
// - Pyro: smoke and fire on a voxel grid (stable fluids with buoyancy,
//   combustion, vorticity confinement and turbulence).
// - RBD: rigid bodies on rapier, a Bullet-style solver.
// - SOP solver: repeatedly cooks a sub-network on the previous step's output.
// - Particles: point emitter with gravity, drag and static-collider bounces.

use super::{flip_cancelled, hash01, Mesh, Vec3};
use crate::vex::perlin3;
use rapier3d::math::{Pose, Vector as RVec};
use rapier3d::prelude::{ColliderBuilder, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

// Simulation steps per second of timeline time, before substeps.
pub const SIM_FPS: f32 = 24.0;
const MAX_SIMULATIONS: usize = 12;
const MAX_CHECKPOINTS: usize = 48;

// The step a solver must reach for `time`; time 0 (or earlier) is the initial state.
pub fn step_count(time: f32, substeps: usize) -> usize {
    (time.max(0.0) * SIM_FPS * substeps.max(1) as f32 + 1e-3).floor() as usize
}

// ============================================================================
// Step cache
// ============================================================================

struct Entry<S, P> {
    live: Option<(usize, S)>,
    // Sorted by step.
    checkpoints: Vec<(usize, P)>,
}

impl<S, P> Entry<S, P> {
    fn new() -> Self {
        Self { live: None, checkpoints: Vec::new() }
    }

    fn checkpoint(&mut self, step: usize, snapshot: P) {
        match self.checkpoints.binary_search_by_key(&step, |(s, _)| *s) {
            Ok(i) => self.checkpoints[i].1 = snapshot,
            Err(i) => self.checkpoints.insert(i, (step, snapshot)),
        }
        if self.checkpoints.len() > MAX_CHECKPOINTS {
            // Drop the checkpoint whose neighbours are closest, keeping the first and last.
            let len = self.checkpoints.len();
            if let Some(i) = (1..len - 1).min_by_key(|&i| self.checkpoints[i + 1].0 - self.checkpoints[i - 1].0) {
                self.checkpoints.remove(i);
            }
        }
    }
}

#[derive(Default)]
struct Store {
    entries: HashMap<u64, Box<dyn Any + Send>>,
    // Least recently used first.
    order: VecDeque<u64>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

// Forgets every cached simulation so the next cook starts from the initial state.
pub fn clear_all() {
    let mut store = store().lock().unwrap_or_else(|e| e.into_inner());
    store.entries.clear();
    store.order.clear();
}

fn take_entry<S: Send + 'static, P: Send + 'static>(key: u64) -> Entry<S, P> {
    let mut store = store().lock().unwrap_or_else(|e| e.into_inner());
    store
        .entries
        .remove(&key)
        .and_then(|boxed| boxed.downcast::<Entry<S, P>>().ok())
        .map(|boxed| *boxed)
        .unwrap_or_else(Entry::new)
}

fn put_entry<S: Send + 'static, P: Send + 'static>(key: u64, entry: Entry<S, P>) {
    let mut store = store().lock().unwrap_or_else(|e| e.into_inner());
    store.entries.insert(key, Box::new(entry));
    store.order.retain(|k| *k != key);
    store.order.push_back(key);
    while store.order.len() > MAX_SIMULATIONS {
        if let Some(oldest) = store.order.pop_front() {
            store.entries.remove(&oldest);
        }
    }
}

// Advances the simulation identified by `key` to step `target` and returns its output.
// The live state is taken out of the cache while stepping, so long simulations never hold the lock.
// A cancelled cook keeps its progress and returns an empty mesh.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run<S: Send + 'static, P: Send + 'static>(
    key: u64,
    target: usize,
    checkpoint_every: usize,
    init: impl FnOnce() -> S,
    snapshot: impl Fn(&S) -> P,
    restore: impl Fn(&P) -> S,
    mut step: impl FnMut(&mut S, usize),
    output: impl FnOnce(&S) -> Mesh,
) -> Mesh {
    let mut entry: Entry<S, P> = take_entry(key);
    let mut start = None;
    if let Some((at, state)) = entry.live.take() {
        let newer_checkpoint = entry.checkpoints.iter().any(|(c, _)| *c > at && *c <= target);
        if at <= target && !newer_checkpoint {
            start = Some((at, state));
        } else {
            // Keep the later state so scrubbing forward again is cheap.
            entry.checkpoint(at, snapshot(&state));
        }
    }
    let (mut at, mut state) = match start {
        Some(found) => found,
        None => match entry.checkpoints.iter().rev().find(|(c, _)| *c <= target) {
            Some((c, p)) => (*c, restore(p)),
            None => (0, init()),
        },
    };
    if at == 0 && entry.checkpoints.first().map_or(true, |(c, _)| *c != 0) {
        entry.checkpoint(0, snapshot(&state));
    }
    while at < target {
        if flip_cancelled() {
            entry.live = Some((at, state));
            put_entry(key, entry);
            return Mesh::default();
        }
        at += 1;
        step(&mut state, at);
        if at % checkpoint_every.max(1) == 0 {
            entry.checkpoint(at, snapshot(&state));
        }
    }
    let mesh = output(&state);
    entry.live = Some((at, state));
    put_entry(key, entry);
    mesh
}

// ============================================================================
// SOP solver
// ============================================================================

// Step 0 is the input geometry; every later step cooks the solver's network on the previous result.
pub fn sop_solver(
    key: u64,
    time: f32,
    substeps: usize,
    init: impl FnOnce() -> Mesh,
    mut step: impl FnMut(&Mesh, f32) -> Mesh,
) -> Mesh {
    let substeps = substeps.clamp(1, 16);
    let rate = SIM_FPS * substeps as f32;
    run(
        key,
        step_count(time, substeps),
        4,
        init,
        |mesh: &Mesh| mesh.clone(),
        |mesh: &Mesh| mesh.clone(),
        |state, at| *state = step(state, at as f32 / rate),
        |mesh| mesh.clone(),
    )
}

// ============================================================================
// Voxel helpers shared by Pyro
// ============================================================================

#[derive(Clone, Copy, Debug)]
struct Grid {
    n: [usize; 3],
    h: f32,
    origin: Vec3,
}

impl Grid {
    fn from_box(center: [f32; 3], size: [f32; 3], resolution: usize) -> Grid {
        let size = size.map(|s| s.max(0.01));
        let h = size.iter().cloned().fold(0.0, f32::max) / resolution.max(4) as f32;
        let n = size.map(|s| ((s / h).ceil() as usize).max(4));
        let origin = Vec3::new(
            center[0] - n[0] as f32 * h * 0.5,
            center[1] - n[1] as f32 * h * 0.5,
            center[2] - n[2] as f32 * h * 0.5,
        );
        Grid { n, h, origin }
    }

    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }

    fn at(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.n[0] * (y + self.n[1] * z)
    }

    fn center(&self, x: usize, y: usize, z: usize) -> Vec3 {
        Vec3::new(
            self.origin.x + (x as f32 + 0.5) * self.h,
            self.origin.y + (y as f32 + 0.5) * self.h,
            self.origin.z + (z as f32 + 0.5) * self.h,
        )
    }

    // Trilinear sample at a world position. Outside the domain reads `outside`
    // when given, otherwise the nearest boundary value.
    fn sample(&self, field: &[f32], p: Vec3, outside: Option<f32>) -> f32 {
        let g = [
            (p.x - self.origin.x) / self.h - 0.5,
            (p.y - self.origin.y) / self.h - 0.5,
            (p.z - self.origin.z) / self.h - 0.5,
        ];
        if let Some(value) = outside {
            if (0..3).any(|a| g[a] < -0.5 || g[a] > self.n[a] as f32 - 0.5) {
                return value;
            }
        }
        let mut base = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for a in 0..3 {
            let c = g[a].clamp(0.0, (self.n[a] - 1) as f32);
            let i = (c.floor() as usize).min(self.n[a] - 2);
            base[a] = i;
            frac[a] = c - i as f32;
        }
        let [x, y, z] = base;
        let [fx, fy, fz] = frac;
        let v = |dx: usize, dy: usize, dz: usize| field[self.at(x + dx, y + dy, z + dz)];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        lerp(
            lerp(lerp(v(0, 0, 0), v(1, 0, 0), fx), lerp(v(0, 1, 0), v(1, 1, 0), fx), fy),
            lerp(lerp(v(0, 0, 1), v(1, 0, 1), fx), lerp(v(0, 1, 1), v(1, 1, 1), fx), fy),
            fz,
        )
    }

    // Voxel index range covering [lo, hi] on axis `a`.
    fn range(&self, a: usize, lo: f32, hi: f32) -> std::ops::Range<usize> {
        let origin = [self.origin.x, self.origin.y, self.origin.z][a];
        let start = ((lo - origin) / self.h - 0.5).ceil().max(0.0) as usize;
        let end = (((hi - origin) / self.h - 0.5).floor() + 1.0).clamp(0.0, self.n[a] as f32) as usize;
        start.min(end)..end
    }
}

// Rasterizes geometry into 0..1 voxel weights: closed meshes fill their inside
// (scanline parity along X) and every point splats a soft sphere, so particles
// and open surfaces still emit.
fn rasterize(grid: &Grid, mesh: &Mesh) -> Vec<f32> {
    let mut weights: Vec<f32> = rasterize_inside(grid, mesh).into_iter().map(|inside| inside as u8 as f32).collect();
    splat_points(grid, mesh, &mut weights);
    weights
}

// Voxels whose centres are inside a closed mesh.
fn rasterize_inside(grid: &Grid, mesh: &Mesh) -> Vec<bool> {
    let mut inside = vec![false; grid.len()];
    let [_, ny, nz] = grid.n;
    if !mesh.indices.is_empty() {
        let mut rows: Vec<Vec<f32>> = vec![Vec::new(); ny * nz];
        for tri in &mesh.indices {
            let [a, b, c] = tri.map(|i| mesh.vertices[i]);
            let d = (b.y - a.y) * (c.z - a.z) - (c.y - a.y) * (b.z - a.z);
            if d.abs() < 1e-12 {
                continue;
            }
            for iz in grid.range(2, a.z.min(b.z).min(c.z), a.z.max(b.z).max(c.z)) {
                for iy in grid.range(1, a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y)) {
                    // Rays pass slightly off the voxel centre so they never hit a shared
                    // triangle edge exactly, which would count one crossing twice.
                    let centre = grid.center(0, iy, iz);
                    let p = Vec3::new(centre.x, centre.y + grid.h * 1.234e-3, centre.z + grid.h * 2.718e-3);
                    let u = ((b.y - p.y) * (c.z - p.z) - (c.y - p.y) * (b.z - p.z)) / d;
                    let v = ((c.y - p.y) * (a.z - p.z) - (a.y - p.y) * (c.z - p.z)) / d;
                    let w = 1.0 - u - v;
                    if u >= 0.0 && v >= 0.0 && w >= 0.0 {
                        rows[iy + ny * iz].push(u * a.x + v * b.x + w * c.x);
                    }
                }
            }
        }
        for iz in 0..nz {
            for iy in 0..ny {
                let row = &mut rows[iy + ny * iz];
                row.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                for pair in row.chunks_exact(2) {
                    for ix in grid.range(0, pair[0], pair[1]) {
                        inside[grid.at(ix, iy, iz)] = true;
                    }
                }
            }
        }
    }
    inside
}

fn splat_points(grid: &Grid, mesh: &Mesh, weights: &mut [f32]) {
    let pscale = mesh.point_attributes.get("pscale").filter(|p| p.len() == mesh.vertices.len());
    for (i, p) in mesh.vertices.iter().enumerate() {
        let radius = pscale.map_or(0.0, |s| s[i]).max(grid.h * 1.5);
        let ranges = [
            grid.range(0, p.x - radius, p.x + radius),
            grid.range(1, p.y - radius, p.y + radius),
            grid.range(2, p.z - radius, p.z + radius),
        ];
        for iz in ranges[2].clone() {
            for iy in ranges[1].clone() {
                for ix in ranges[0].clone() {
                    let falloff = 1.0 - grid.center(ix, iy, iz).sub(*p).length() / radius;
                    if falloff > 0.0 {
                        let cell = &mut weights[grid.at(ix, iy, iz)];
                        *cell = cell.max(falloff.min(1.0));
                    }
                }
            }
        }
    }
}

// ============================================================================
// Pyro
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct PyroSettings {
    // Voxels along the longest side of the domain.
    pub resolution: usize,
    pub center: [f32; 3],
    pub size: [f32; 3],
    pub substeps: usize,
    // Source amounts written where the source geometry is.
    pub density: f32,
    pub temperature: f32,
    pub fuel: f32,
    pub buoyancy: f32,
    // Smoke loss and heat loss per second.
    pub dissipation: f32,
    pub cooling: f32,
    pub vorticity: f32,
    pub turbulence: f32,
    pub burn_rate: f32,
    pub wind: [f32; 3],
    // Display opacity of the smoke.
    pub opacity: f32,
}

impl Default for PyroSettings {
    fn default() -> Self {
        Self {
            resolution: 40,
            center: [0.0, 2.0, 0.0],
            size: [4.0, 5.0, 4.0],
            substeps: 1,
            density: 1.0,
            temperature: 1.0,
            fuel: 0.0,
            buoyancy: 1.0,
            dissipation: 0.15,
            cooling: 0.8,
            vorticity: 0.4,
            turbulence: 0.3,
            burn_rate: 1.0,
            wind: [0.0; 3],
            opacity: 1.0,
        }
    }
}

impl PyroSettings {
    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.resolution as f32];
        p.extend(self.center);
        p.extend(self.size);
        p.extend([
            self.substeps as f32,
            self.density,
            self.temperature,
            self.fuel,
            self.buoyancy,
            self.dissipation,
            self.cooling,
            self.vorticity,
            self.turbulence,
            self.burn_rate,
        ]);
        p.extend(self.wind);
        p.push(self.opacity);
        p
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::default();
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        let v3 = |i: usize, default: [f32; 3]| [g(i, default[0]), g(i + 1, default[1]), g(i + 2, default[2])];
        Self {
            resolution: (g(0, d.resolution as f32) as usize).clamp(8, 160),
            center: v3(1, d.center),
            size: v3(4, d.size).map(|s| s.max(0.1)),
            substeps: (g(7, 1.0) as usize).clamp(1, 8),
            density: g(8, d.density).max(0.0),
            temperature: g(9, d.temperature).max(0.0),
            fuel: g(10, d.fuel).max(0.0),
            buoyancy: g(11, d.buoyancy),
            dissipation: g(12, d.dissipation).max(0.0),
            cooling: g(13, d.cooling).max(0.0),
            vorticity: g(14, d.vorticity).max(0.0),
            turbulence: g(15, d.turbulence).max(0.0),
            burn_rate: g(16, d.burn_rate).max(0.0),
            wind: v3(17, d.wind),
            opacity: g(20, d.opacity).max(0.0),
        }
    }

    // (label, settings) presets that keep the domain and resolution.
    pub fn presets(&self) -> [(&'static str, PyroSettings); 3] {
        let keep = |s: PyroSettings| PyroSettings { resolution: self.resolution, center: self.center, size: self.size, substeps: self.substeps, wind: self.wind, ..s };
        [
            ("Smoke", keep(PyroSettings::default())),
            (
                "Fire",
                keep(PyroSettings {
                    density: 0.15,
                    temperature: 0.6,
                    fuel: 1.0,
                    buoyancy: 1.3,
                    dissipation: 0.4,
                    cooling: 1.4,
                    vorticity: 0.7,
                    turbulence: 0.5,
                    burn_rate: 1.2,
                    ..PyroSettings::default()
                }),
            ),
            (
                "Thick smoke",
                keep(PyroSettings { density: 2.0, temperature: 0.7, buoyancy: 0.6, dissipation: 0.02, cooling: 0.4, opacity: 1.5, ..PyroSettings::default() }),
            ),
        ]
    }
}

#[derive(Clone)]
struct PyroState {
    grid: Grid,
    u: Vec<f32>,
    v: Vec<f32>,
    w: Vec<f32>,
    density: Vec<f32>,
    temperature: Vec<f32>,
    fuel: Vec<f32>,
    flame: Vec<f32>,
    solid: Arc<Vec<bool>>,
    // Rasterized source of a static source; empty until the first step, unused when animated.
    source: Arc<Vec<f32>>,
}

impl PyroState {
    fn new(grid: Grid, solid: Vec<bool>) -> Self {
        let zeros = vec![0.0; grid.len()];
        Self {
            grid,
            u: zeros.clone(),
            v: zeros.clone(),
            w: zeros.clone(),
            density: zeros.clone(),
            temperature: zeros.clone(),
            fuel: zeros.clone(),
            flame: zeros,
            solid: Arc::new(solid),
            source: Arc::new(Vec::new()),
        }
    }
}

const PRESSURE_ITERATIONS: usize = 30;

fn pyro_step(s: &mut PyroState, set: &PyroSettings, dt: f32, time: f32, animated_source: Option<&[f32]>) {
    let g = s.grid;
    let [nx, ny, nz] = g.n;
    let n = g.len();
    let solid = s.solid.clone();
    let static_source = s.source.clone();
    let source: &[f32] = animated_source.unwrap_or(&static_source);

    // Sources write the maximum of the current and source values, like Pyro's "maximum" mode.
    for (i, &weight) in source.iter().enumerate() {
        if weight > 0.0 && !solid[i] {
            s.density[i] = s.density[i].max(weight * set.density);
            s.temperature[i] = s.temperature[i].max(weight * set.temperature);
            s.fuel[i] = s.fuel[i].max(weight * set.fuel);
        }
    }

    // Combustion: hot fuel burns into heat, soot and flame.
    let flame_decay = (-10.0 * dt).exp();
    for i in 0..n {
        let mut flame = 0.0;
        if s.fuel[i] > 1e-4 && s.temperature[i] > 0.05 && set.burn_rate > 0.0 {
            let burn = s.fuel[i].min(set.burn_rate * dt * 3.0);
            s.fuel[i] -= burn;
            s.temperature[i] += burn * 2.5;
            s.density[i] += burn * 0.6;
            flame = (burn / (dt * 3.0)).min(1.5);
        }
        s.flame[i] = (s.flame[i] * flame_decay).max(flame);
    }

    // Forces: buoyancy, wind and turbulence.
    let wind = Vec3::from_arr(set.wind);
    let wind_blend = if wind.length() > 0.0 { 1.0 - (-0.5 * dt).exp() } else { 0.0 };
    let frequency = 2.5 / set.size.iter().cloned().fold(0.1, f32::max);
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                if solid[i] {
                    continue;
                }
                s.v[i] += dt * (set.buoyancy * 4.0 * s.temperature[i] - 0.4 * s.density[i]);
                if wind_blend > 0.0 {
                    s.u[i] += (wind.x - s.u[i]) * wind_blend;
                    s.v[i] += (wind.y - s.v[i]) * wind_blend;
                    s.w[i] += (wind.z - s.w[i]) * wind_blend;
                }
                if set.turbulence > 0.0 && s.density[i] + s.temperature[i] > 0.01 {
                    let p = g.center(x, y, z);
                    let q = [p.x * frequency + time * 0.3, p.y * frequency - time * 0.6, p.z * frequency + time * 0.2];
                    let push = set.turbulence * 6.0 * dt;
                    s.u[i] += perlin3(q, 11) * push;
                    s.v[i] += perlin3(q, 23) * push;
                    s.w[i] += perlin3(q, 37) * push;
                }
            }
        }
    }

    if set.vorticity > 0.0 {
        vorticity_confinement(s, set.vorticity * 3.0, dt);
    }
    project(s);
    advect(s, dt);

    let keep_smoke = (-set.dissipation * dt).exp();
    let keep_heat = (-set.cooling * dt).exp();
    for i in 0..n {
        if solid[i] {
            s.u[i] = 0.0;
            s.v[i] = 0.0;
            s.w[i] = 0.0;
            s.density[i] = 0.0;
            s.temperature[i] = 0.0;
            s.fuel[i] = 0.0;
            s.flame[i] = 0.0;
            continue;
        }
        s.density[i] *= keep_smoke;
        s.temperature[i] *= keep_heat;
        for field in [&mut s.density[i], &mut s.temperature[i], &mut s.fuel[i], &mut s.flame[i]] {
            if *field < 1e-5 {
                *field = 0.0;
            }
        }
    }
}

fn vorticity_confinement(s: &mut PyroState, epsilon: f32, dt: f32) {
    let g = s.grid;
    let [nx, ny, nz] = g.n;
    let inv = 1.0 / (2.0 * g.h);
    let mut curl = vec![[0.0f32; 3]; g.len()];
    let mut magnitude = vec![0.0f32; g.len()];
    for z in 1..nz - 1 {
        for y in 1..ny - 1 {
            for x in 1..nx - 1 {
                let i = g.at(x, y, z);
                let (xp, xm) = (g.at(x + 1, y, z), g.at(x - 1, y, z));
                let (yp, ym) = (g.at(x, y + 1, z), g.at(x, y - 1, z));
                let (zp, zm) = (g.at(x, y, z + 1), g.at(x, y, z - 1));
                let c = [
                    (s.w[yp] - s.w[ym] - (s.v[zp] - s.v[zm])) * inv,
                    (s.u[zp] - s.u[zm] - (s.w[xp] - s.w[xm])) * inv,
                    (s.v[xp] - s.v[xm] - (s.u[yp] - s.u[ym])) * inv,
                ];
                curl[i] = c;
                magnitude[i] = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            }
        }
    }
    for z in 1..nz - 1 {
        for y in 1..ny - 1 {
            for x in 1..nx - 1 {
                let i = g.at(x, y, z);
                if s.solid[i] || magnitude[i] < 1e-6 {
                    continue;
                }
                let gradient = Vec3::new(
                    magnitude[g.at(x + 1, y, z)] - magnitude[g.at(x - 1, y, z)],
                    magnitude[g.at(x, y + 1, z)] - magnitude[g.at(x, y - 1, z)],
                    magnitude[g.at(x, y, z + 1)] - magnitude[g.at(x, y, z - 1)],
                );
                let normal = gradient.normalize();
                let force = normal.cross(Vec3::from_arr(curl[i])).scale(epsilon * g.h * dt);
                s.u[i] += force.x;
                s.v[i] += force.y;
                s.w[i] += force.z;
            }
        }
    }
}

// Makes the velocity divergence-free. Side walls and the floor are closed, the top is open.
fn project(s: &mut PyroState) {
    let g = s.grid;
    let [nx, ny, nz] = g.n;
    let n = g.len();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                if x == 0 || x == nx - 1 {
                    s.u[i] = 0.0;
                }
                if y == 0 {
                    s.v[i] = 0.0;
                }
                if z == 0 || z == nz - 1 {
                    s.w[i] = 0.0;
                }
            }
        }
    }
    let solid = s.solid.clone();
    // Neighbour index along an axis, or None outside the domain.
    let neighbour = |x: usize, y: usize, z: usize, axis: usize, forward: bool| -> Option<usize> {
        let mut c = [x as isize, y as isize, z as isize];
        c[axis] += if forward { 1 } else { -1 };
        (c.iter().zip(g.n.iter()).all(|(v, n)| *v >= 0 && (*v as usize) < *n)).then(|| g.at(c[0] as usize, c[1] as usize, c[2] as usize))
    };
    let inv = 1.0 / (2.0 * g.h);
    let mut divergence = vec![0.0f32; n];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                if solid[i] {
                    continue;
                }
                let mut div = 0.0;
                for (axis, field) in [(0, &s.u), (1, &s.v), (2, &s.w)] {
                    let value = |forward: bool| match neighbour(x, y, z, axis, forward) {
                        Some(j) if !solid[j] => field[j],
                        Some(_) => 0.0,
                        // Open top: outflow keeps its velocity.
                        None if axis == 1 && forward => field[i],
                        None => 0.0,
                    };
                    div += value(true) - value(false);
                }
                divergence[i] = div * inv;
            }
        }
    }
    let h2 = g.h * g.h;
    let mut pressure = vec![0.0f32; n];
    let mut next = vec![0.0f32; n];
    for _ in 0..PRESSURE_ITERATIONS {
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let i = g.at(x, y, z);
                    if solid[i] {
                        continue;
                    }
                    let mut sum = 0.0;
                    for axis in 0..3 {
                        for forward in [true, false] {
                            sum += match neighbour(x, y, z, axis, forward) {
                                Some(j) if !solid[j] => pressure[j],
                                Some(_) => pressure[i],
                                None if axis == 1 && forward => 0.0,
                                None => pressure[i],
                            };
                        }
                    }
                    next[i] = (sum - divergence[i] * h2) / 6.0;
                }
            }
        }
        std::mem::swap(&mut pressure, &mut next);
    }
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                if solid[i] {
                    continue;
                }
                let p = |axis: usize, forward: bool| match neighbour(x, y, z, axis, forward) {
                    Some(j) if !solid[j] => pressure[j],
                    Some(_) => pressure[i],
                    None if axis == 1 && forward => 0.0,
                    None => pressure[i],
                };
                s.u[i] -= (p(0, true) - p(0, false)) * inv;
                s.v[i] -= (p(1, true) - p(1, false)) * inv;
                s.w[i] -= (p(2, true) - p(2, false)) * inv;
            }
        }
    }
}

// Semi-Lagrangian advection with a midpoint backtrace. Smoke entering from outside the domain is empty.
fn advect(s: &mut PyroState, dt: f32) {
    let g = s.grid;
    let [nx, ny, nz] = g.n;
    let n = g.len();
    let mut out: [Vec<f32>; 7] = std::array::from_fn(|_| vec![0.0; n]);
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                if s.solid[i] {
                    continue;
                }
                let p = g.center(x, y, z);
                let velocity = Vec3::new(s.u[i], s.v[i], s.w[i]);
                let mid = p.sub(velocity.scale(dt * 0.5));
                let velocity = Vec3::new(g.sample(&s.u, mid, None), g.sample(&s.v, mid, None), g.sample(&s.w, mid, None));
                let back = p.sub(velocity.scale(dt));
                out[0][i] = g.sample(&s.u, back, None);
                out[1][i] = g.sample(&s.v, back, None);
                out[2][i] = g.sample(&s.w, back, None);
                out[3][i] = g.sample(&s.density, back, Some(0.0));
                out[4][i] = g.sample(&s.temperature, back, Some(0.0));
                out[5][i] = g.sample(&s.fuel, back, Some(0.0));
                out[6][i] = g.sample(&s.flame, back, Some(0.0));
            }
        }
    }
    let [u, v, w, density, temperature, fuel, flame] = out;
    (s.u, s.v, s.w, s.density, s.temperature, s.fuel, s.flame) = (u, v, w, density, temperature, fuel, flame);
}

// Emission strength of Pyro fire in MoonRay renders (radiance per unit flame).
const PYRO_EMISSION: f32 = 6.0;

fn fire_color(flame: f32) -> [f32; 3] {
    let t = (flame / 1.2).clamp(0.0, 1.0);
    let ramp = [[0.55, 0.06, 0.01], [1.0, 0.35, 0.04], [1.0, 0.75, 0.25], [1.0, 0.95, 0.75]];
    let f = t * (ramp.len() - 1) as f32;
    let i = (f.floor() as usize).min(ramp.len() - 2);
    let k = f - i as f32;
    std::array::from_fn(|c| ramp[i][c] + (ramp[i + 1][c] - ramp[i][c]) * k)
}

// A light shading smoke in the real-time renderer: shining from a direction, from a position
// (fading with distance like the viewport's sphere lights), or from all around (a dome).
pub enum VolumeLight {
    Distant { to_light: Vec3, color: [f32; 3], intensity: f32 },
    Sphere { position: Vec3, color: [f32; 3], intensity: f32, falloff: f32 },
    Dome { color: [f32; 3], intensity: f32 },
}

// Smoke's display colour (Cd) lit by the scene's lights, for volume points (Pyro, VDB) that
// carry their density: each point's light is dimmed by the smoke between it and the light,
// marched through a grid rebuilt from the points. Flame is unchanged: it glows by itself.
// None when the mesh isn't a volume or is too big to grid. Without lights the solver's own
// shading (lit from straight overhead) stays, so this is only used when the scene has lights.
pub fn relight_smoke(mesh: &Mesh, lights: &[VolumeLight]) -> Option<Vec<f32>> {
    let n = mesh.vertices.len();
    let attr = |name: &str| mesh.point_attributes.get(name).filter(|v| v.len() == n && n > 0);
    let density = attr("density")?;
    let flame = attr("flame");
    let h = *mesh.point_attributes.get("detail:volume_voxelsize")?.first()?;
    let opacity = mesh.point_attributes.get("detail:volume_extinction").and_then(|e| e.first()).map_or(1.0, |e| e * h);
    if h <= 0.0 {
        return None;
    }
    let origin = mesh.point_attributes.get("detail:volume_origin").filter(|o| o.len() == 3).map_or(Vec3::zero(), |o| Vec3::new(o[0], o[1], o[2]));
    // The voxel each point sits in (points are jittered inside their voxel).
    let key = |p: Vec3| [((p.x - origin.x) / h).round() as i64, ((p.y - origin.y) / h).round() as i64, ((p.z - origin.z) / h).round() as i64];
    let keys: Vec<[i64; 3]> = mesh.vertices.iter().map(|p| key(*p)).collect();
    let lo: [i64; 3] = std::array::from_fn(|a| keys.iter().map(|k| k[a]).min().unwrap_or(0));
    let dims: [usize; 3] = std::array::from_fn(|a| (keys.iter().map(|k| k[a]).max().unwrap_or(0) - lo[a] + 1) as usize);
    if dims.iter().product::<usize>() > 8_000_000 {
        return None;
    }
    let at = |k: [i64; 3]| -> Option<usize> {
        let r: [i64; 3] = std::array::from_fn(|a| k[a] - lo[a]);
        (0..3).all(|a| r[a] >= 0 && (r[a] as usize) < dims[a]).then(|| (r[2] as usize * dims[1] + r[1] as usize) * dims[0] + r[0] as usize)
    };
    let mut grid = vec![0.0f32; dims.iter().product()];
    for (k, d) in keys.iter().zip(density) {
        if let Some(i) = at(*k) {
            grid[i] += d;
        }
    }
    let longest = dims.iter().copied().max().unwrap_or(1) as f32 * 1.8;
    // How much light gets from the voxel at `p` to it along `dir`, through the smoke.
    let through = |p: Vec3, dir: Vec3, until: f32| -> f32 {
        let mut optical_depth = 0.0;
        let steps = (until / h).min(longest) as usize;
        for s in 1..=steps {
            match at(key(p.add(dir.scale(s as f32 * h)))) {
                Some(i) => optical_depth += grid[i] * h * 6.0 * opacity,
                None => break,
            }
        }
        (-optical_depth).exp()
    };
    let mut out = Vec::with_capacity(n * 3);
    for (i, p) in mesh.vertices.iter().enumerate() {
        let mut light = [0.0f32; 3];
        for l in lights {
            let (amount, color) = match l {
                VolumeLight::Distant { to_light, color, intensity } => (intensity * through(*p, *to_light, f32::INFINITY), color),
                VolumeLight::Sphere { position, color, intensity, falloff } => {
                    let to = position.sub(*p);
                    let d = to.length().max(1e-4);
                    (intensity * (falloff / (d * d)).min(1.5) * through(*p, to.scale(1.0 / d), d), color)
                }
                VolumeLight::Dome { color, intensity } => (intensity * 0.5, color),
            };
            (0..3).for_each(|c| light[c] += amount * color[c]);
        }
        let smoke: [f32; 3] = std::array::from_fn(|c| ((0.22 + 0.7 * light[c]) * if c == 2 { 1.03 } else { 1.0 }).min(1.0));
        let f = flame.map_or(0.0, |v| v[i]);
        let fire_weight = (f * 1.2).clamp(0.0, 1.0);
        let fire = fire_color(f);
        out.extend((0..3).map(|c| (smoke[c] * (1.0 - fire_weight) + fire[c] * fire_weight * 1.3).min(1.0)));
    }
    Some(out)
}

// Voxels with smoke or flame become points carrying density, temperature, flame,
// a display colour (Cd), opacity (Alpha) and splat size (pscale).
fn pyro_output(s: &PyroState, set: &PyroSettings) -> Mesh {
    let g = s.grid;
    let [nx, ny, nz] = g.n;
    // Self-shadowing from a light overhead: transmittance accumulated down each column.
    let mut shadow = vec![1.0f32; g.len()];
    for z in 0..nz {
        for x in 0..nx {
            let mut optical_depth = 0.0f32;
            for y in (0..ny).rev() {
                let i = g.at(x, y, z);
                shadow[i] = (-optical_depth).exp();
                optical_depth += s.density[i] * g.h * 6.0 * set.opacity;
            }
        }
    }
    let mut mesh = Mesh::default();
    let names = ["density", "temperature", "flame", "Alpha", "pscale"];
    let mut columns: [Vec<f32>; 5] = Default::default();
    let mut colors = Vec::new();
    let mut emission = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = g.at(x, y, z);
                let (density, flame) = (s.density[i], s.flame[i]);
                if density < 0.01 && flame < 0.02 {
                    continue;
                }
                // Jitter inside the voxel so the grid pattern does not show.
                let jitter = |salt: u32| (super::hash01(i as u32, salt) - 0.5) * g.h * 0.7;
                let c = g.center(x, y, z);
                mesh.vertices.push(Vec3::new(c.x + jitter(3), c.y + jitter(5), c.z + jitter(7)));
                let brightness = 0.22 + 0.7 * shadow[i];
                let smoke = [brightness, brightness, brightness * 1.03];
                let fire_weight = (flame * 1.2).clamp(0.0, 1.0);
                let fire = fire_color(flame);
                let color: [f32; 3] = std::array::from_fn(|c| (smoke[c] * (1.0 - fire_weight) + fire[c] * fire_weight * 1.3).min(1.0));
                let alpha = (1.0 - (-(density * set.opacity * 1.5 + flame * 2.0)).exp()).clamp(0.0, 0.95);
                for (column, value) in columns.iter_mut().zip([density, s.temperature[i], flame, alpha, g.h * 0.85]) {
                    column.push(value);
                }
                colors.extend(color);
                // Fire glows in renders: blackbody-like colour scaled by the flame amount.
                emission.extend(fire.map(|c| c * flame));
            }
        }
    }
    for (name, values) in names.iter().zip(columns) {
        mesh.point_attributes.insert(name.to_string(), values);
    }
    mesh.point_attributes.insert("Cd".into(), colors);
    mesh.point_attributes.insert("emission".into(), emission);
    // Render density per world unit matching the viewport's per-voxel opacity, and fire brightness.
    mesh.point_attributes.insert("detail:volume_extinction".into(), vec![set.opacity / g.h]);
    mesh.point_attributes.insert("detail:volume_emission".into(), vec![PYRO_EMISSION]);
    // Describes the grid so Convert VDB can treat the smoke as a fog volume.
    let origin = g.center(0, 0, 0);
    mesh.point_attributes.insert("detail:volume_voxelsize".into(), vec![g.h]);
    mesh.point_attributes.insert("detail:volume_origin".into(), vec![origin.x, origin.y, origin.z]);
    mesh.point_attributes.insert("detail:volume_class".into(), vec![1.0]);
    mesh.point_attributes.insert("detail:volume_background".into(), vec![0.0]);
    mesh
}

pub fn pyro(
    key: u64,
    settings: &PyroSettings,
    time: f32,
    animated_source: bool,
    mut source_at: impl FnMut(f32) -> Mesh,
    collider: impl FnOnce() -> Mesh,
) -> Mesh {
    let substeps = settings.substeps.clamp(1, 8);
    let dt = 1.0 / (SIM_FPS * substeps as f32);
    run(
        key,
        step_count(time, substeps),
        6,
        || {
            let grid = Grid::from_box(settings.center, settings.size, settings.resolution);
            // Only the inside of closed collider meshes blocks the gas.
            let solid = rasterize_inside(&grid, &collider());
            PyroState::new(grid, solid)
        },
        |state: &PyroState| state.clone(),
        |state: &PyroState| state.clone(),
        |state, at| {
            let t = at as f32 * dt;
            if !animated_source && state.source.is_empty() {
                state.source = Arc::new(rasterize(&state.grid, &source_at(0.0)));
            }
            let field = animated_source.then(|| rasterize(&state.grid, &source_at(t)));
            pyro_step(state, settings, dt, t, field.as_deref());
        },
        |state| pyro_output(state, settings),
    )
}

// ============================================================================
// RBD (rapier)
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct RbdSettings {
    // 0 convex hull, 1 convex decomposition, 2 box, 3 sphere.
    pub shape: usize,
    pub density: f32,
    pub friction: f32,
    pub bounce: f32,
    pub gravity: f32,
    pub substeps: usize,
    pub ground: bool,
    pub ground_height: f32,
    pub velocity: [f32; 3],
    // Initial angular velocity in radians per second.
    pub spin: [f32; 3],
}

impl Default for RbdSettings {
    fn default() -> Self {
        Self {
            shape: 0,
            density: 1.0,
            friction: 0.6,
            bounce: 0.2,
            gravity: 9.81,
            substeps: 2,
            ground: true,
            ground_height: 0.0,
            velocity: [0.0; 3],
            spin: [0.0; 3],
        }
    }
}

impl RbdSettings {
    pub const SHAPES: [&'static str; 4] = ["Convex hull", "Convex decomposition", "Box", "Sphere"];

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![
            self.shape as f32,
            self.density,
            self.friction,
            self.bounce,
            self.gravity,
            self.substeps as f32,
            self.ground as u8 as f32,
            self.ground_height,
        ];
        p.extend(self.velocity);
        p.extend(self.spin);
        p
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::default();
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        let v3 = |i: usize| [g(i, 0.0), g(i + 1, 0.0), g(i + 2, 0.0)];
        Self {
            shape: (g(0, 0.0) as usize).min(3),
            density: g(1, d.density).max(0.001),
            friction: g(2, d.friction).max(0.0),
            bounce: g(3, d.bounce).clamp(0.0, 1.0),
            gravity: g(4, d.gravity),
            substeps: (g(5, d.substeps as f32) as usize).clamp(1, 16),
            ground: g(6, 1.0) != 0.0,
            ground_height: g(7, 0.0),
            velocity: v3(8),
            spin: v3(11),
        }
    }
}

struct Piece {
    points: Vec<usize>,
    // Point positions relative to `origin`.
    local: Vec<RVec>,
    tris: Vec<[u32; 3]>,
    origin: Vec3,
    initial_velocity: Vec3,
}

struct RbdBase {
    mesh: Mesh,
    collider: Mesh,
    pieces: Vec<Piece>,
}

fn rv(v: Vec3) -> RVec {
    RVec::new(v.x, v.y, v.z)
}

impl RbdBase {
    // Each connected part is a rigid body, or each value of a `piece` point attribute when present.
    fn new(mesh: Mesh, collider: Mesh) -> Self {
        let n = mesh.vertices.len();
        let labels: Vec<usize> = match mesh.point_attributes.get("piece").filter(|v| v.len() == n) {
            Some(piece) => {
                let mut ids = HashMap::new();
                piece.iter().map(|v| { let next = ids.len(); *ids.entry(v.round() as i64).or_insert(next) }).collect()
            }
            None => {
                let mut parent: Vec<usize> = (0..n).collect();
                fn find(parent: &mut [usize], mut i: usize) -> usize {
                    while parent[i] != i {
                        parent[i] = parent[parent[i]];
                        i = parent[i];
                    }
                    i
                }
                for tri in &mesh.indices {
                    for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2])] {
                        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                        parent[ra] = rb;
                    }
                }
                let mut ids = HashMap::new();
                (0..n).map(|i| { let root = find(&mut parent, i); let next = ids.len(); *ids.entry(root).or_insert(next) }).collect()
            }
        };
        let count = labels.iter().max().map_or(0, |m| m + 1);
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (point, label) in labels.iter().enumerate() {
            groups[*label].push(point);
        }
        let mut local_index = vec![0u32; n];
        for group in &groups {
            for (i, point) in group.iter().enumerate() {
                local_index[*point] = i as u32;
            }
        }
        let mut tris: Vec<Vec<[u32; 3]>> = vec![Vec::new(); count];
        for tri in &mesh.indices {
            let label = labels[tri[0]];
            if tri.iter().all(|p| labels[*p] == label) {
                tris[label].push(tri.map(|p| local_index[p]));
            }
        }
        let velocity = mesh.point_attributes.get("v").filter(|v| v.len() == n * 3);
        let pieces = groups
            .into_iter()
            .zip(tris)
            .filter(|(points, _)| !points.is_empty())
            .map(|(points, tris)| {
                let origin = points.iter().fold(Vec3::zero(), |sum, p| sum.add(mesh.vertices[*p])).scale(1.0 / points.len() as f32);
                let initial_velocity = velocity.map_or(Vec3::zero(), |v| {
                    points.iter().fold(Vec3::zero(), |sum, p| sum.add(Vec3::new(v[p * 3], v[p * 3 + 1], v[p * 3 + 2]))).scale(1.0 / points.len() as f32)
                });
                let local = points.iter().map(|p| rv(mesh.vertices[*p].sub(origin))).collect();
                Piece { points, local, tris, origin, initial_velocity }
            })
            .collect();
        Self { mesh, collider, pieces }
    }
}

fn piece_collider(piece: &Piece, shape: usize, point_radius: f32) -> ColliderBuilder {
    let mut lo = RVec::splat(f32::INFINITY);
    let mut hi = RVec::splat(f32::NEG_INFINITY);
    for p in &piece.local {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let half = ((hi - lo) * 0.5).max(RVec::splat(0.005));
    let middle = (hi + lo) * 0.5;
    let cuboid = || ColliderBuilder::cuboid(half.x, half.y, half.z).translation(middle);
    let radius = piece.local.iter().map(|p| p.length()).fold(0.0f32, f32::max);
    if piece.local.len() < 4 {
        return ColliderBuilder::ball(radius.max(point_radius));
    }
    // Flat pieces have no volume for a hull; a thin box works instead.
    let flat = (hi - lo).min_element() < 1e-3;
    let guarded = |build: &dyn Fn() -> Option<ColliderBuilder>| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)).ok().flatten()
    };
    match shape {
        0 if !flat => guarded(&|| ColliderBuilder::convex_hull(&piece.local)).unwrap_or_else(cuboid),
        1 if !flat && !piece.tris.is_empty() => {
            guarded(&|| Some(ColliderBuilder::convex_decomposition(&piece.local, &piece.tris))).unwrap_or_else(cuboid)
        }
        3 => ColliderBuilder::ball(radius.max(0.005)),
        _ => cuboid(),
    }
}

#[derive(Clone, Copy)]
struct BodySnap {
    pose: Pose,
    linvel: RVec,
    angvel: RVec,
}

struct RbdState {
    base: Arc<RbdBase>,
    world: PhysicsWorld,
    handles: Vec<RigidBodyHandle>,
}

#[derive(Clone)]
struct RbdSnapshot {
    base: Arc<RbdBase>,
    bodies: Vec<BodySnap>,
}

fn build_world(base: &Arc<RbdBase>, set: &RbdSettings, snaps: Option<&[BodySnap]>) -> RbdState {
    let mut world = PhysicsWorld::new();
    world.gravity = RVec::new(0.0, -set.gravity, 0.0);
    world.integration_parameters.dt = 1.0 / (SIM_FPS * set.substeps.clamp(1, 16) as f32);
    if set.ground {
        world.colliders.insert(
            ColliderBuilder::cuboid(1000.0, 0.5, 1000.0)
                .translation(RVec::new(0.0, set.ground_height - 0.5, 0.0))
                .friction(set.friction)
                .restitution(set.bounce),
        );
    }
    if !base.collider.indices.is_empty() {
        let vertices = base.collider.vertices.iter().map(|v| rv(*v)).collect();
        let indices = base.collider.indices.iter().map(|t| t.map(|i| i as u32)).collect();
        if let Ok(collider) = ColliderBuilder::trimesh(vertices, indices) {
            world.colliders.insert(collider.friction(set.friction).restitution(set.bounce));
        }
    }
    let point_radius = 0.05;
    let handles = base
        .pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            let body = RigidBodyBuilder::dynamic().ccd_enabled(true);
            let body = match snaps.and_then(|s| s.get(i)) {
                Some(snap) => body.pose(snap.pose).linvel(snap.linvel).angvel(snap.angvel),
                None => body
                    .translation(rv(piece.origin))
                    .linvel(rv(piece.initial_velocity.add(Vec3::from_arr(set.velocity))))
                    .angvel(RVec::from_array(set.spin)),
            };
            let handle = world.bodies.insert(body);
            let collider = piece_collider(piece, set.shape, point_radius)
                .density(set.density.max(0.001))
                .friction(set.friction)
                .restitution(set.bounce);
            world.colliders.insert_with_parent(collider, handle, &mut world.bodies);
            handle
        })
        .collect();
    RbdState { base: base.clone(), world, handles }
}

fn rbd_output(state: &RbdState) -> Mesh {
    let mut mesh = state.base.mesh.clone();
    let n = mesh.vertices.len();
    let mut velocity = vec![0.0f32; n * 3];
    let mut piece_ids = vec![0.0f32; n];
    for (index, (piece, handle)) in state.base.pieces.iter().zip(&state.handles).enumerate() {
        let body = &state.world.bodies[*handle];
        let pose = *body.position();
        let (linvel, angvel) = (body.linvel(), body.angvel());
        for (&point, local) in piece.points.iter().zip(&piece.local) {
            let p = pose.transform_point(*local);
            mesh.vertices[point] = Vec3::new(p.x, p.y, p.z);
            let v = linvel + angvel.cross(p - pose.translation);
            velocity[point * 3..point * 3 + 3].copy_from_slice(&[v.x, v.y, v.z]);
            piece_ids[point] = index as f32;
        }
    }
    mesh.point_attributes.insert("v".into(), velocity);
    mesh.point_attributes.insert("piece".into(), piece_ids);
    mesh
}

pub fn rbd(
    key: u64,
    settings: &RbdSettings,
    time: f32,
    pieces: impl FnOnce() -> Mesh,
    collider: impl FnOnce() -> Mesh,
) -> Mesh {
    run(
        key,
        step_count(time, settings.substeps.clamp(1, 16)),
        12,
        || build_world(&Arc::new(RbdBase::new(pieces(), collider())), settings, None),
        |state: &RbdState| RbdSnapshot {
            base: state.base.clone(),
            bodies: state
                .handles
                .iter()
                .map(|h| {
                    let body = &state.world.bodies[*h];
                    BodySnap { pose: *body.position(), linvel: body.linvel(), angvel: body.angvel() }
                })
                .collect(),
        },
        |snap: &RbdSnapshot| build_world(&snap.base, settings, Some(&snap.bodies)),
        |state, _| state.world.step(),
        rbd_output,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_is_lit_by_the_scenes_lights_through_itself() {
        // A slab of smoke 10 voxels across x, with a burning voxel in the middle.
        let h = 0.1;
        let mut m = Mesh::default();
        let (mut density, mut flame) = (Vec::new(), Vec::new());
        for x in 0..10 {
            for y in 0..3 {
                m.vertices.push(Vec3::new(x as f32 * h, y as f32 * h, 0.0));
                density.push(1.0);
                flame.push(if x == 5 && y == 1 { 1.0 } else { 0.0 });
            }
        }
        m.point_attributes.insert("density".into(), density);
        m.point_attributes.insert("flame".into(), flame);
        m.point_attributes.insert("detail:volume_voxelsize".into(), vec![h]);
        m.point_attributes.insert("detail:volume_origin".into(), vec![0.0; 3]);
        m.point_attributes.insert("detail:volume_extinction".into(), vec![1.0 / h]);
        let red = |x: usize, cd: &[f32]| cd[(x * 3 + 1) * 3];
        let from = |dir: Vec3| relight_smoke(&m, &[VolumeLight::Distant { to_light: dir, color: [1.0; 3], intensity: 1.0 }]).unwrap();
        // Lit from +x: the +x side is bright, the far side in the slab's shadow.
        let right = from(Vec3::new(1.0, 0.0, 0.0));
        assert!(red(9, &right) > red(0, &right) + 0.3, "{} {}", red(9, &right), red(0, &right));
        let left = from(Vec3::new(-1.0, 0.0, 0.0));
        assert!(red(0, &left) > red(9, &left) + 0.3);
        // The burning voxel glows the same either way.
        assert_eq!(&right[(5 * 3 + 1) * 3..(5 * 3 + 1) * 3 + 3], &left[(5 * 3 + 1) * 3..(5 * 3 + 1) * 3 + 3]);
        // A dome lights every voxel alike, with no shadow.
        let dome = relight_smoke(&m, &[VolumeLight::Dome { color: [1.0; 3], intensity: 1.0 }]).unwrap();
        assert_eq!(red(0, &dome), red(9, &dome));
        // A coloured light tints the smoke.
        let blue = relight_smoke(&m, &[VolumeLight::Distant { to_light: Vec3::new(1.0, 0.0, 0.0), color: [0.2, 0.3, 1.0], intensity: 1.0 }]).unwrap();
        let lit = (9 * 3 + 1) * 3;
        assert!(blue[lit + 2] > blue[lit], "blue more than red on the lit side");
        // Geometry that isn't a volume is left alone.
        let mut plain = Mesh::default();
        plain.vertices.push(Vec3::zero());
        assert!(relight_smoke(&plain, &[]).is_none());
    }

    fn cube(center: Vec3, half: f32) -> Mesh {
        let mut mesh = Mesh::default();
        for i in 0..8 {
            let corner = |bit: usize| if i & bit != 0 { half } else { -half };
            mesh.vertices.push(center.add(Vec3::new(corner(1), corner(2), corner(4))));
        }
        let faces = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        for [a, b, c, d] in faces {
            mesh.indices.push([a, b, c]);
            mesh.indices.push([a, c, d]);
        }
        mesh
    }

    #[test]
    fn step_cache_resumes_and_rewinds() {
        clear_all();
        let calls = std::cell::Cell::new(0);
        let mut simulate = |target: usize| {
            run(
                9001,
                target,
                5,
                || 0usize,
                |s: &usize| *s,
                |s: &usize| *s,
                |s, _| {
                    *s += 1;
                    calls.set(calls.get() + 1);
                },
                |s| Mesh { vertices: vec![Vec3::new(*s as f32, 0.0, 0.0)], ..Default::default() },
            )
            .vertices[0]
            .x
        };
        assert_eq!(simulate(10), 10.0);
        assert_eq!(calls.get(), 10);
        // Forward continues from the live state.
        assert_eq!(simulate(12), 12.0);
        assert_eq!(calls.get(), 12);
        // Back resumes from the checkpoint at step 5.
        assert_eq!(simulate(7), 7.0);
        assert_eq!(calls.get(), 14);
    }

    #[test]
    fn rigid_cube_falls_and_rests_on_the_ground() {
        clear_all();
        let settings = RbdSettings::default();
        let start = cube(Vec3::new(0.0, 2.0, 0.0), 0.5);
        let at_rest = rbd(1, &settings, 3.0, || start.clone(), Mesh::default);
        let lowest = at_rest.vertices.iter().map(|v| v.y).fold(f32::INFINITY, f32::min);
        assert!(lowest.abs() < 0.05, "cube should rest on the ground, lowest point {lowest}");
        assert_eq!(at_rest.point_attributes["piece"].len(), 8);
        let initial = rbd(1, &settings, 0.0, || start.clone(), Mesh::default);
        assert!((initial.vertices[0].y - 1.5).abs() < 1e-5, "time 0 is the input");
    }

    #[test]
    fn separate_pieces_become_separate_bodies() {
        let mut two = cube(Vec3::new(-2.0, 1.0, 0.0), 0.4);
        let other = cube(Vec3::new(2.0, 1.0, 0.0), 0.4);
        let offset = two.vertices.len();
        two.vertices.extend(other.vertices);
        two.indices.extend(other.indices.iter().map(|t| t.map(|i| i + offset)));
        let base = RbdBase::new(two, Mesh::default());
        assert_eq!(base.pieces.len(), 2);
    }

    #[test]
    fn smoke_rises_from_its_source() {
        clear_all();
        let settings = PyroSettings { resolution: 24, ..PyroSettings::default() };
        let source = cube(Vec3::new(0.0, 0.3, 0.0), 0.4);
        let out = pyro(2, &settings, 1.0, false, |_| source.clone(), Mesh::default);
        assert!(!out.vertices.is_empty());
        let average_y = out.vertices.iter().map(|v| v.y).sum::<f32>() / out.vertices.len() as f32;
        assert!(average_y > 0.5, "smoke should rise above the source, average height {average_y}");
        for name in ["density", "Cd", "Alpha", "pscale"] {
            assert!(out.point_attributes.contains_key(name), "{name}");
        }
    }

    #[test]
    fn fire_burns_fuel_into_flame() {
        clear_all();
        let settings = PyroSettings { resolution: 20, fuel: 1.0, ..PyroSettings::default() };
        let source = cube(Vec3::new(0.0, 0.3, 0.0), 0.4);
        let out = pyro(3, &settings, 0.5, false, |_| source.clone(), Mesh::default);
        assert!(out.point_attributes["flame"].iter().any(|f| *f > 0.2));
    }

    #[test]
    fn solid_inside_a_closed_mesh() {
        let grid = Grid::from_box([0.0; 3], [2.0; 3], 10);
        let solid = rasterize_inside(&grid, &cube(Vec3::zero(), 0.5));
        let count = solid.iter().filter(|s| **s).count();
        assert!(count > 0 && count < grid.len());
        assert!(solid[grid.at(5, 5, 5)]);
        assert!(!solid[grid.at(0, 0, 0)]);
    }

    #[test]
    fn settings_round_trip() {
        let pyro = PyroSettings { fuel: 0.7, wind: [1.0, 0.0, -2.0], ..PyroSettings::default() };
        assert_eq!(PyroSettings::from_params(&pyro.to_params()), pyro);
        let rbd = RbdSettings { shape: 2, ground: false, spin: [0.0, 3.0, 0.0], ..RbdSettings::default() };
        assert_eq!(RbdSettings::from_params(&rbd.to_params()), rbd);
    }
}

// ============================================================================
// Particles
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleSettings {
    // Most particles alive at once.
    pub max_particles: usize,
    // Particles born per second.
    pub rate: f32,
    pub lifetime: f32,
    pub speed: f32,
    // Width of the emission cone (0 = straight along the emission direction).
    pub spread: f32,
    pub gravity: f32,
    pub drag: f32,
    // Fraction of normal speed kept when bouncing off the collider.
    pub bounce: f32,
    // Fraction of tangential speed lost on each collision.
    pub friction: f32,
    // Collision radius of each particle.
    pub radius: f32,
    pub substeps: usize,
    // Emit along the source's surface normals instead of straight up.
    pub along_normals: bool,
    pub seed: u32,
}

#[derive(Clone, Default)]
struct ParticleState {
    p: Vec<Vec3>,
    v: Vec<Vec3>,
    age: Vec<f32>,
    life: Vec<f32>,
    id: Vec<u32>,
    next_id: u32,
    // Fractional births carried over between steps.
    pending: f32,
}

// Where new particles are born: area-weighted triangles, else points, else the origin.
struct Emitter {
    mesh: Mesh,
    // Cumulative triangle areas for area-weighted sampling.
    cumulative: Vec<f32>,
}

impl Emitter {
    fn new(mesh: Mesh) -> Self {
        let mut total = 0.0;
        let cumulative = mesh
            .indices
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| mesh.vertices[i]);
                total += b.sub(a).cross(c.sub(a)).length() * 0.5;
                total
            })
            .collect();
        Emitter { mesh, cumulative }
    }

    // A birth position, emission normal and inherited velocity for particle `id`.
    fn sample(&self, id: u32, seed: u32) -> (Vec3, Vec3, Vec3) {
        let r = |k: u32| hash01(id.wrapping_mul(13).wrapping_add(k), seed);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let total = self.cumulative.last().copied().unwrap_or(0.0);
        if total > 0.0 {
            let target = r(1) * total;
            let t = self.cumulative.partition_point(|&c| c < target).min(self.mesh.indices.len() - 1);
            let [a, b, c] = self.mesh.indices[t].map(|i| self.mesh.vertices[i]);
            let (mut u, mut v) = (r(2), r(3));
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let p = a.add(b.sub(a).scale(u)).add(c.sub(a).scale(v));
            let n = b.sub(a).cross(c.sub(a));
            let n = if n.length() > 1e-12 { n.normalize() } else { up };
            return (p, n, Vec3::new(0.0, 0.0, 0.0));
        }
        let count = self.mesh.vertices.len();
        if count > 0 {
            let i = ((r(1) * count as f32) as usize).min(count - 1);
            let read = |name: &str| {
                self.mesh
                    .point_attributes
                    .get(name)
                    .filter(|a| a.len() == count * 3)
                    .map(|a| Vec3::new(a[i * 3], a[i * 3 + 1], a[i * 3 + 2]))
            };
            let n = read("N").filter(|n| n.length() > 1e-12).map(|n| n.normalize()).unwrap_or(up);
            return (self.mesh.vertices[i], n, read("v").unwrap_or(Vec3::new(0.0, 0.0, 0.0)));
        }
        (Vec3::new(0.0, 0.0, 0.0), up, Vec3::new(0.0, 0.0, 0.0))
    }
}

// Unit vector inside a cone of half-width ~`spread` around `axis`.
fn cone_direction(axis: Vec3, spread: f32, a: f32, b: f32) -> Vec3 {
    let helper = if axis.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
    let tangent = axis.cross(helper).normalize();
    let bitangent = axis.cross(tangent);
    let theta = a * std::f32::consts::TAU;
    let radial = b.sqrt() * spread;
    axis.add(tangent.scale(radial * theta.cos())).add(bitangent.scale(radial * theta.sin())).normalize()
}

fn particle_step(
    s: &mut ParticleState,
    set: &ParticleSettings,
    dt: f32,
    emitter: &Emitter,
    collider: Option<&crate::volume::Volume>,
) {
    // Age and kill.
    for age in &mut s.age {
        *age += dt;
    }
    let mut i = 0;
    while i < s.p.len() {
        if s.age[i] >= s.life[i] {
            s.p.swap_remove(i);
            s.v.swap_remove(i);
            s.age.swap_remove(i);
            s.life.swap_remove(i);
            s.id.swap_remove(i);
        } else {
            i += 1;
        }
    }
    // Birth.
    s.pending += set.rate.max(0.0) * dt;
    while s.pending >= 1.0 {
        s.pending -= 1.0;
        if s.p.len() >= set.max_particles {
            continue;
        }
        let id = s.next_id;
        s.next_id = s.next_id.wrapping_add(1);
        let (p, normal, inherited) = emitter.sample(id, set.seed);
        let axis = if set.along_normals { normal } else { Vec3::new(0.0, 1.0, 0.0) };
        let r = |k: u32| hash01(id.wrapping_mul(13).wrapping_add(k), set.seed ^ 0x5bd1_e995);
        let dir = cone_direction(axis, set.spread, r(4), r(5));
        let speed = set.speed * (0.75 + 0.5 * r(6));
        s.p.push(p);
        s.v.push(inherited.add(dir.scale(speed)));
        s.age.push(0.0);
        s.life.push(set.lifetime.max(0.01) * (0.8 + 0.4 * r(7)));
        s.id.push(id);
    }
    // Forces, motion and collisions.
    let keep = (1.0 - set.drag.max(0.0) * dt).max(0.0);
    for k in 0..s.p.len() {
        let mut v = s.v[k];
        v.y += set.gravity * dt;
        v = v.scale(keep);
        let mut p = s.p[k].add(v.scale(dt));
        if let Some(sdf) = collider {
            let phi = sdf.sample(p);
            if phi < set.radius {
                let g = sdf.gradient(p);
                if g.length() > 1e-6 {
                    let n = g.normalize();
                    p = p.add(n.scale(set.radius - phi));
                    let vn = v.dot(n);
                    if vn < 0.0 {
                        let normal = n.scale(vn);
                        let tangent = v.sub(normal);
                        v = tangent.scale(1.0 - set.friction.clamp(0.0, 1.0)).sub(normal.scale(set.bounce.clamp(0.0, 1.0)));
                    }
                }
            }
        }
        s.p[k] = p;
        s.v[k] = v;
    }
}

fn particle_output(s: &ParticleState) -> Mesh {
    let mut mesh = Mesh { vertices: s.p.clone(), ..Mesh::default() };
    mesh.point_attributes.insert("v".into(), s.v.iter().flat_map(|v| [v.x, v.y, v.z]).collect());
    mesh.point_attributes.insert("age".into(), s.age.clone());
    mesh.point_attributes.insert("life".into(), s.life.clone());
    mesh.point_attributes.insert("id".into(), s.id.iter().map(|&i| i as f32).collect());
    mesh
}

// Voxel size for a collider's distance field: ~96 voxels across its largest side.
// A static collider's signed distance field, with distances filled in all the
// way through (so deep penetrations still get pushed out). Built once per
// collider shape and shared, since every cook of a solver asks for it again.
pub(crate) fn collider_field(collider: &Mesh) -> Option<Arc<crate::volume::Volume>> {
    use std::hash::{Hash, Hasher};
    let (lo, hi) = collider.bounds().filter(|_| !collider.indices.is_empty())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for v in &collider.vertices {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()].hash(&mut hasher);
    }
    collider.indices.hash(&mut hasher);
    let key = hasher.finish();
    static CACHE: OnceLock<Mutex<VecDeque<(u64, Arc<crate::volume::Volume>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, field)) = cache.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Some(field.clone());
    }
    let extent = hi.sub(lo);
    let voxel = (extent.x.max(extent.y).max(extent.z) / 96.0).max(1e-3);
    let mut field = crate::volume::from_polygons(collider, voxel, 4.0).ok()?;
    crate::volume::fill_distances(&mut field);
    let field = Arc::new(field);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.push_back((key, field.clone()));
    if cache.len() > 4 {
        cache.pop_front();
    }
    Some(field)
}

pub fn particles(
    key: u64,
    settings: &ParticleSettings,
    time: f32,
    animated_source: bool,
    mut source_at: impl FnMut(f32) -> Mesh,
    collider: impl FnOnce() -> Mesh,
) -> Mesh {
    let substeps = settings.substeps.clamp(1, 8);
    let dt = 1.0 / (SIM_FPS * substeps as f32);
    // Built on the first step that actually runs, so cached frames cost nothing.
    let mut collider = Some(collider);
    let mut field: Option<Option<Arc<crate::volume::Volume>>> = None;
    let mut fixed_emitter: Option<Emitter> = None;
    run(
        key,
        step_count(time, substeps),
        12,
        ParticleState::default,
        |state: &ParticleState| state.clone(),
        |state: &ParticleState| state.clone(),
        |state, at| {
            let field = field.get_or_insert_with(|| collider.take().and_then(|c| collider_field(&c())));
            let emitter = if animated_source {
                fixed_emitter.insert(Emitter::new(source_at(at as f32 * dt)))
            } else {
                fixed_emitter.get_or_insert_with(|| Emitter::new(source_at(0.0)))
            };
            particle_step(state, settings, dt, emitter, field.as_deref());
        },
        particle_output,
    )
}

#[cfg(test)]
mod particle_tests {
    use super::*;

    fn settings() -> ParticleSettings {
        ParticleSettings {
            max_particles: 500,
            rate: 200.0,
            lifetime: 10.0,
            speed: 0.0,
            spread: 0.0,
            gravity: -9.8,
            drag: 0.0,
            bounce: 0.5,
            friction: 0.0,
            radius: 0.02,
            substeps: 2,
            along_normals: false,
            seed: 1,
        }
    }

    fn quad(y: f32, size: f32) -> Mesh {
        let mut mesh = Mesh::default();
        mesh.vertices = vec![
            Vec3::new(-size, y, -size),
            Vec3::new(size, y, -size),
            Vec3::new(size, y, size),
            Vec3::new(-size, y, size),
        ];
        mesh.indices = vec![[0, 2, 1], [0, 3, 2]];
        mesh
    }

    #[test]
    fn emits_from_source_and_lands_on_collider() {
        clear_all();
        let source = quad(1.0, 0.5);
        let ground = quad(0.0, 3.0);
        let out = particles(9001, &settings(), 3.0, false, |_| source.clone(), || ground.clone());
        assert!(!out.vertices.is_empty());
        assert!(out.vertices.len() <= 500);
        // Everything was born on the source square and has fallen onto the ground.
        for p in &out.vertices {
            assert!(p.x.abs() <= 0.5 + 1e-3 && p.z.abs() <= 0.5 + 1e-3, "{p:?} left the source footprint");
            assert!(p.y > -0.05 && p.y < 0.3, "{p:?} should rest on the ground");
        }
        assert_eq!(out.point_attributes["v"].len(), out.vertices.len() * 3);
        assert_eq!(out.point_attributes["age"].len(), out.vertices.len());
    }

    #[test]
    fn falls_freely_without_collider() {
        clear_all();
        let out = particles(9002, &settings(), 1.0, false, |_| Mesh::default(), Mesh::default);
        // Emitted at the origin with no collider: the oldest particle has fallen ~g/2 * t^2.
        let lowest = out.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        assert!(lowest < -4.0, "lowest particle at {lowest}");
    }

    #[test]
    fn scrubbing_back_reproduces_the_same_frame() {
        clear_all();
        let source = quad(1.0, 0.5);
        let first = particles(9003, &settings(), 0.5, false, |_| source.clone(), Mesh::default);
        let _later = particles(9003, &settings(), 2.0, false, |_| source.clone(), Mesh::default);
        let again = particles(9003, &settings(), 0.5, false, |_| source.clone(), Mesh::default);
        assert_eq!(first.vertices, again.vertices);
    }
}
