// Terrain: height grids and the nodes that shape them.
//
// A terrain is an ordinary grid mesh (points row by row along x, then z, with
// height in P.y) carrying `detail:terrain` = [columns, rows, spacing x, spacing z],
// so later nodes can work on it as a heightfield. Nodes that take a mask read
// the `mask` point attribute (Terrain Mask writes it; Attribute Paint can too).

use super::{hash01, Mesh, Vec3};

pub const INFO: &str = "detail:terrain";

pub const NOISE_TYPES: [&str; 5] = ["fBm (rolling hills)", "Ridged (mountain ridges)", "Billow (puffy)", "Cellular (crags)", "Warped fBm (eroded look)"];
pub const BLEND_MODES: [&str; 5] = ["Add", "Replace", "Multiply", "Maximum", "Minimum"];
pub const MASK_SOURCES: [&str; 4] = ["Height", "Slope", "Erosion flow", "Erosion debris"];
pub const MASK_COMBINE: [&str; 5] = ["Replace", "Multiply", "Add", "Maximum", "Minimum"];

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainSettings {
    pub size: [f32; 2],
    // Grid cells along the longer side.
    pub resolution: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoiseSettings {
    pub kind: usize,
    pub blend: usize,
    pub amplitude: f32,
    // Size of the largest features, in world units.
    pub element_size: f32,
    pub octaves: usize,
    // Amplitude kept per octave.
    pub roughness: f32,
    pub offset: [f32; 2],
    pub seed: u32,
    // Raise the noise so its lowest point sits at 0 (no negative valleys).
    pub positive: bool,
    pub use_mask: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErodeSettings {
    // Hydraulic: raindrops that pick up and drop sediment.
    pub droplets: usize,
    pub erosion: f32,
    pub deposition: f32,
    pub capacity: f32,
    pub evaporation: f32,
    pub inertia: f32,
    pub radius: usize,
    // Thermal: material slides down slopes steeper than the talus angle.
    pub thermal_iterations: usize,
    pub talus_angle: f32,
    pub seed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerraceSettings {
    pub step: f32,
    pub offset: f32,
    // 0 = no change, 1 = hard flat steps.
    pub sharpness: f32,
    pub use_mask: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaskSettings {
    pub source: usize,
    // The range that maps to 1 (heights in world units, slopes in degrees,
    // flow and debris in their own units).
    pub min: f32,
    pub max: f32,
    pub falloff: f32,
    pub blur: usize,
    pub invert: bool,
    pub combine: usize,
    pub visualize: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorizeSettings {
    pub sand: [f32; 3],
    pub grass: [f32; 3],
    pub rock: [f32; 3],
    pub snow: [f32; 3],
    // Heights from 0 (lowest) to 1 (highest point).
    pub sand_level: f32,
    pub snow_level: f32,
    // Slopes steeper than this (degrees) are rock.
    pub rock_slope: f32,
    pub blend: f32,
}

impl NoiseSettings {
    pub fn defaults() -> Self {
        NoiseSettings {
            kind: 0,
            blend: 0,
            amplitude: 1.5,
            element_size: 4.0,
            octaves: 6,
            roughness: 0.5,
            offset: [0.0, 0.0],
            seed: 1,
            positive: true,
            use_mask: false,
        }
    }
}

impl ErodeSettings {
    pub fn defaults() -> Self {
        ErodeSettings {
            droplets: 50_000,
            erosion: 0.3,
            deposition: 0.3,
            capacity: 4.0,
            evaporation: 0.02,
            inertia: 0.05,
            radius: 2,
            thermal_iterations: 10,
            talus_angle: 40.0,
            seed: 1,
        }
    }
}

impl ColorizeSettings {
    pub fn defaults() -> Self {
        ColorizeSettings {
            sand: [0.76, 0.70, 0.50],
            grass: [0.24, 0.40, 0.16],
            rock: [0.42, 0.38, 0.34],
            snow: [0.95, 0.96, 0.98],
            sand_level: 0.08,
            snow_level: 0.75,
            rock_slope: 35.0,
            blend: 0.05,
        }
    }
}

// The grid behind a terrain mesh.
#[derive(Clone, Copy, Debug)]
struct Grid {
    nx: usize,
    nz: usize,
    dx: f32,
    dz: f32,
}

fn grid_of(mesh: &Mesh) -> Result<Grid, String> {
    let info = mesh.point_attributes.get(INFO).filter(|i| i.len() >= 4).ok_or(
        "the input isn't a terrain: start from a Terrain node (Terrain menu)",
    )?;
    let grid = Grid { nx: info[0] as usize, nz: info[1] as usize, dx: info[2], dz: info[3] };
    if grid.nx < 2 || grid.nz < 2 || grid.nx * grid.nz != mesh.vertices.len() {
        return Err("the terrain's points were added or removed since the Terrain node, so it's no longer a grid".into());
    }
    Ok(grid)
}

fn heights(mesh: &Mesh) -> Vec<f32> {
    mesh.vertices.iter().map(|v| v.y).collect()
}

// Writes heights back; old normals no longer match.
fn set_heights(mut mesh: Mesh, h: &[f32]) -> Mesh {
    for (v, y) in mesh.vertices.iter_mut().zip(h) {
        v.y = *y;
    }
    mesh.point_attributes.remove("N");
    mesh
}

fn mask_of(mesh: &Mesh, enabled: bool) -> Option<Vec<f32>> {
    enabled
        .then(|| mesh.point_attributes.get("mask").filter(|m| m.len() == mesh.vertices.len()).cloned())
        .flatten()
}

pub fn terrain(set: &TerrainSettings) -> Mesh {
    let (sx, sz) = (set.size[0].max(1e-3), set.size[1].max(1e-3));
    let cells = set.resolution.clamp(2, 2048) as f32;
    let spacing = sx.max(sz) / cells;
    let nx = ((sx / spacing).round() as usize).max(1) + 1;
    let nz = ((sz / spacing).round() as usize).max(1) + 1;
    let mut mesh = super::node_grid(sx, sz, nx, nz);
    mesh.point_attributes.insert(INFO.into(), vec![nx as f32, nz as f32, sx / (nx - 1) as f32, sz / (nz - 1) as f32]);
    mesh
}

// --- Noise -------------------------------------------------------------------

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lattice(x: i32, z: i32, seed: u32) -> u32 {
    (x as u32).wrapping_mul(0x8da6_b343) ^ (z as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f)
}

// 2D gradient (Perlin) noise in about [-1, 1].
fn perlin(x: f32, z: f32, seed: u32) -> f32 {
    let (x0, z0) = (x.floor(), z.floor());
    let (fx, fz) = (x - x0, z - z0);
    let (ix, iz) = (x0 as i32, z0 as i32);
    let grad = |i: i32, k: i32, px: f32, pz: f32| {
        let a = hash01(lattice(i, k, seed), seed) * std::f32::consts::TAU;
        a.cos() * px + a.sin() * pz
    };
    let n00 = grad(ix, iz, fx, fz);
    let n10 = grad(ix + 1, iz, fx - 1.0, fz);
    let n01 = grad(ix, iz + 1, fx, fz - 1.0);
    let n11 = grad(ix + 1, iz + 1, fx - 1.0, fz - 1.0);
    let (u, v) = (fade(fx), fade(fz));
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.414
}

// Distance to the nearest feature point (0 at the point, about 1 between cells).
fn cellular(x: f32, z: f32, seed: u32) -> f32 {
    let (ix, iz) = (x.floor() as i32, z.floor() as i32);
    let mut best = f32::MAX;
    for k in -1..=1 {
        for i in -1..=1 {
            let h = lattice(ix + i, iz + k, seed);
            let px = (ix + i) as f32 + hash01(h, 11);
            let pz = (iz + k) as f32 + hash01(h, 23);
            best = best.min((px - x).powi(2) + (pz - z).powi(2));
        }
    }
    best.sqrt()
}

fn fractal(set: &NoiseSettings, x: f32, z: f32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    // Ridged noise feeds each octave's ridges into the next one's weight.
    let mut weight = 1.0f32;
    for o in 0..set.octaves.clamp(1, 12) {
        let seed = set.seed.wrapping_add(o as u32 * 101);
        let (px, pz) = (x * freq, z * freq);
        let n = match set.kind {
            1 => {
                let r = 1.0 - perlin(px, pz, seed).abs();
                let r = r * r * weight;
                weight = (r * 2.0).clamp(0.0, 1.0);
                r
            }
            2 => perlin(px, pz, seed).abs() * 2.0 - 1.0,
            3 => 1.0 - cellular(px, pz, seed),
            _ => perlin(px, pz, seed),
        };
        sum += n * amp;
        norm += amp;
        amp *= set.roughness.clamp(0.0, 1.0);
        freq *= 2.0;
    }
    sum / norm.max(1e-6)
}

fn sample_noise(set: &NoiseSettings, x: f32, z: f32) -> f32 {
    let s = 1.0 / set.element_size.max(1e-3);
    let (x, z) = ((x + set.offset[0]) * s, (z + set.offset[1]) * s);
    if set.kind == 4 {
        // Domain warp: offset the lookup by another noise for flowing, eroded shapes.
        let base = NoiseSettings { kind: 0, ..set.clone() };
        let wx = fractal(&base, x + 5.2, z + 1.3);
        let wz = fractal(&NoiseSettings { seed: set.seed ^ 0x5bd1, ..base.clone() }, x + 1.7, z + 9.2);
        fractal(&base, x + 1.5 * wx, z + 1.5 * wz)
    } else {
        fractal(set, x, z)
    }
}

pub fn terrain_noise(input: &Mesh, set: &NoiseSettings) -> Result<Mesh, String> {
    grid_of(input)?;
    let mut noise: Vec<f32> = input.vertices.iter().map(|v| sample_noise(set, v.x, v.z)).collect();
    if set.positive {
        let lo = noise.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = noise.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let span = (hi - lo).max(1e-6);
        noise.iter_mut().for_each(|n| *n = (*n - lo) / span);
    }
    let mask = mask_of(input, set.use_mask);
    let h: Vec<f32> = heights(input)
        .iter()
        .zip(&noise)
        .enumerate()
        .map(|(i, (h, n))| {
            let n = n * set.amplitude;
            let v = match set.blend {
                1 => n,
                2 => h * n,
                3 => h.max(n),
                4 => h.min(n),
                _ => h + n,
            };
            let m = mask.as_ref().map_or(1.0, |m| m[i].clamp(0.0, 1.0));
            h + (v - h) * m
        })
        .collect();
    Ok(set_heights(input.clone(), &h))
}

// --- Erosion -----------------------------------------------------------------

// Height and downhill gradient at a position given in cells.
fn height_and_gradient(h: &[f32], g: &Grid, x: f32, z: f32) -> (f32, f32, f32) {
    let (ix, iz) = (x.floor() as usize, z.floor() as usize);
    let (u, v) = (x - ix as f32, z - iz as f32);
    let i = iz * g.nx + ix;
    let (h00, h10, h01, h11) = (h[i], h[i + 1], h[i + g.nx], h[i + g.nx + 1]);
    let gx = (h10 - h00) * (1.0 - v) + (h11 - h01) * v;
    let gz = (h01 - h00) * (1.0 - u) + (h11 - h10) * u;
    let height = h00 * (1.0 - u) * (1.0 - v) + h10 * u * (1.0 - v) + h01 * (1.0 - u) * v + h11 * u * v;
    (height, gx, gz)
}

// Droplet erosion, after Hans Beyer's "Implementation of a method for hydraulic erosion".
fn hydraulic(h: &mut [f32], g: &Grid, set: &ErodeSettings, flow: &mut [f32], debris: &mut [f32]) {
    let radius = set.radius.clamp(1, 8) as i32;
    // Erosion brush: weights for the cells within `radius`.
    let mut brush = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let d = ((dx * dx + dz * dz) as f32).sqrt();
            if d <= radius as f32 {
                brush.push((dx, dz, 1.0 - d / (radius as f32 + 1.0)));
            }
        }
    }
    let total: f32 = brush.iter().map(|b| b.2).sum();
    brush.iter_mut().for_each(|b| b.2 /= total);
    // Heights are in world units but steps are in cells: scale slopes to match.
    let cell = (g.dx + g.dz) * 0.5;
    let max_x = (g.nx - 1) as f32 - 1e-3;
    let max_z = (g.nz - 1) as f32 - 1e-3;
    for d in 0..set.droplets.min(5_000_000) as u32 {
        let mut x = hash01(d.wrapping_mul(2), set.seed) * max_x;
        let mut z = hash01(d.wrapping_mul(2) + 1, set.seed) * max_z;
        let (mut dirx, mut dirz) = (0.0f32, 0.0f32);
        let (mut speed, mut water, mut sediment) = (1.0f32, 1.0f32, 0.0f32);
        // Where the droplet last was on the map, so it can drop what it still carries.
        let mut last = (x as usize, z as usize, 0.0f32, 0.0f32);
        for _ in 0..64 {
            let (ix, iz) = (x as usize, z as usize);
            let (u, v) = (x - ix as f32, z - iz as f32);
            last = (ix, iz, u, v);
            let (height, gx, gz) = height_and_gradient(h, g, x, z);
            dirx = dirx * set.inertia - gx * (1.0 - set.inertia);
            dirz = dirz * set.inertia - gz * (1.0 - set.inertia);
            let len = (dirx * dirx + dirz * dirz).sqrt();
            if len < 1e-9 {
                break;
            }
            dirx /= len;
            dirz /= len;
            x += dirx;
            z += dirz;
            if !(0.0..max_x).contains(&x) || !(0.0..max_z).contains(&z) {
                break;
            }
            let i = iz * g.nx + ix;
            flow[i] += water;
            let (new_height, _, _) = height_and_gradient(h, g, x, z);
            let dh = new_height - height;
            let capacity = (-dh / cell).max(0.01) * speed * water * set.capacity * cell;
            if sediment > capacity || dh > 0.0 {
                // Uphill: fill the pit behind. Otherwise drop the excess.
                let drop = if dh > 0.0 { dh.min(sediment) } else { (sediment - capacity) * set.deposition };
                sediment -= drop;
                for (k, w) in [(i, (1.0 - u) * (1.0 - v)), (i + 1, u * (1.0 - v)), (i + g.nx, (1.0 - u) * v), (i + g.nx + 1, u * v)] {
                    h[k] += drop * w;
                    debris[k] += drop * w;
                }
            } else {
                let take = ((capacity - sediment) * set.erosion).min(-dh);
                for (bx, bz, w) in &brush {
                    let (cx, cz) = (ix as i32 + bx, iz as i32 + bz);
                    if cx < 0 || cz < 0 || cx >= g.nx as i32 || cz >= g.nz as i32 {
                        continue;
                    }
                    let k = cz as usize * g.nx + cx as usize;
                    // Never dig below where the droplet goes next: that makes pits that feed on themselves.
                    let amount = (take * w).min((h[k] - new_height).max(0.0));
                    h[k] -= amount;
                    sediment += amount;
                }
            }
            speed = (speed * speed + dh.abs() / cell * 4.0).sqrt().min(8.0);
            water *= 1.0 - set.evaporation.clamp(0.0, 1.0);
            if water < 0.01 {
                break;
            }
        }
        let (ix, iz, u, v) = last;
        let i = iz * g.nx + ix;
        for (k, w) in [(i, (1.0 - u) * (1.0 - v)), (i + 1, u * (1.0 - v)), (i + g.nx, (1.0 - u) * v), (i + g.nx + 1, u * v)] {
            h[k] += sediment * w;
            debris[k] += sediment * w;
        }
    }
}

// Moves material from cells steeper than the talus angle to their lower neighbours.
fn thermal(h: &mut [f32], g: &Grid, set: &ErodeSettings, debris: &mut [f32]) {
    let talus = set.talus_angle.clamp(1.0, 89.0).to_radians().tan();
    let neighbours: [(i32, i32, f32); 4] = [(1, 0, g.dx), (-1, 0, g.dx), (0, 1, g.dz), (0, -1, g.dz)];
    for _ in 0..set.thermal_iterations.min(500) {
        let mut delta = vec![0.0f32; h.len()];
        for z in 0..g.nz as i32 {
            for x in 0..g.nx as i32 {
                let i = z as usize * g.nx + x as usize;
                let mut moves = [(0usize, 0.0f32); 4];
                let (mut total, mut steepest) = (0.0, 0.0f32);
                for (k, (ox, oz, dist)) in neighbours.iter().enumerate() {
                    let (cx, cz) = (x + ox, z + oz);
                    if cx < 0 || cz < 0 || cx >= g.nx as i32 || cz >= g.nz as i32 {
                        continue;
                    }
                    let j = cz as usize * g.nx + cx as usize;
                    let excess = h[i] - h[j] - talus * dist;
                    if excess > 0.0 {
                        moves[k] = (j, excess);
                        total += excess;
                        steepest = steepest.max(excess);
                    }
                }
                if total <= 0.0 {
                    continue;
                }
                // Move half the steepest excess, shared by how far below each neighbour is.
                let amount = steepest * 0.5 * 0.5;
                delta[i] -= amount;
                for (j, excess) in moves.iter().filter(|m| m.1 > 0.0) {
                    delta[*j] += amount * excess / total;
                }
            }
        }
        for (i, d) in delta.iter().enumerate() {
            h[i] += d;
            if *d > 0.0 {
                debris[i] += d;
            }
        }
    }
}

pub fn terrain_erode(input: &Mesh, set: &ErodeSettings) -> Result<Mesh, String> {
    let g = grid_of(input)?;
    let mut h = heights(input);
    let mut flow = vec![0.0; h.len()];
    let mut debris = vec![0.0; h.len()];
    if set.droplets > 0 {
        hydraulic(&mut h, &g, set, &mut flow, &mut debris);
    }
    if set.thermal_iterations > 0 {
        thermal(&mut h, &g, set, &mut debris);
    }
    // Flow spans orders of magnitude: store it log-scaled to 0..1.
    let peak = flow.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let flow: Vec<f32> = flow.iter().map(|f| (1.0 + f).ln() / (1.0 + peak).ln()).collect();
    let mut out = set_heights(input.clone(), &h);
    out.point_attributes.insert("flow".into(), flow);
    out.point_attributes.insert("debris".into(), debris);
    Ok(out)
}

// --- Terrace -----------------------------------------------------------------

pub fn terrain_terrace(input: &Mesh, set: &TerraceSettings) -> Result<Mesh, String> {
    grid_of(input)?;
    let step = set.step.max(1e-4);
    let sharp = set.sharpness.clamp(0.0, 1.0);
    let mask = mask_of(input, set.use_mask);
    let h: Vec<f32> = heights(input)
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let level = (h - set.offset) / step;
            let base = level.floor();
            let t = level - base;
            // Flat for the first `sharp` part of each step, then a ramp up to the next.
            let ramp = if sharp >= 1.0 { 0.0 } else { ((t - sharp) / (1.0 - sharp)).max(0.0) };
            let ramp = ramp * ramp * (3.0 - 2.0 * ramp);
            let v = (base + ramp) * step + set.offset;
            let m = mask.as_ref().map_or(1.0, |m| m[i].clamp(0.0, 1.0));
            h + (v - h) * m
        })
        .collect();
    Ok(set_heights(input.clone(), &h))
}

// --- Mask --------------------------------------------------------------------

// Slope in degrees at every point, from central differences.
fn slopes(h: &[f32], g: &Grid) -> Vec<f32> {
    let mut out = vec![0.0; h.len()];
    for z in 0..g.nz {
        for x in 0..g.nx {
            let at = |x: usize, z: usize| h[z * g.nx + x];
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(g.nx - 1));
            let (z0, z1) = (z.saturating_sub(1), (z + 1).min(g.nz - 1));
            let sx = (at(x1, z) - at(x0, z)) / ((x1 - x0) as f32 * g.dx);
            let sz = (at(x, z1) - at(x, z0)) / ((z1 - z0) as f32 * g.dz);
            out[z * g.nx + x] = (sx * sx + sz * sz).sqrt().atan().to_degrees();
        }
    }
    out
}

fn blur(values: &mut Vec<f32>, g: &Grid, passes: usize) {
    for _ in 0..passes.min(100) {
        let src = values.clone();
        for z in 0..g.nz {
            for x in 0..g.nx {
                let (mut sum, mut n) = (0.0, 0.0);
                for (ox, oz) in [(0i32, 0i32), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (cx, cz) = (x as i32 + ox, z as i32 + oz);
                    if cx >= 0 && cz >= 0 && cx < g.nx as i32 && cz < g.nz as i32 {
                        sum += src[cz as usize * g.nx + cx as usize];
                        n += 1.0;
                    }
                }
                values[z * g.nx + x] = sum / n;
            }
        }
    }
}

// Black-to-white preview of a 0..1 mask, tinted red where it's 1.
fn mask_colour(m: f32) -> [f32; 3] {
    let m = m.clamp(0.0, 1.0);
    [0.15 + 0.85 * m, 0.15 + 0.35 * m, 0.15 + 0.2 * m]
}

pub fn terrain_mask(input: &Mesh, set: &MaskSettings) -> Result<Mesh, String> {
    let g = grid_of(input)?;
    let h = heights(input);
    let source = match set.source {
        1 => slopes(&h, &g),
        2 | 3 => {
            let name = if set.source == 2 { "flow" } else { "debris" };
            input
                .point_attributes
                .get(name)
                .filter(|v| v.len() == h.len())
                .cloned()
                .ok_or_else(|| format!("the input has no `{name}` attribute: put a Terrain Erode before this node"))?
        }
        _ => h,
    };
    let (lo, hi) = (set.min.min(set.max), set.min.max(set.max));
    let fall = set.falloff.max(0.0);
    let mut mask: Vec<f32> = source
        .iter()
        .map(|v| {
            let m = if *v >= lo && *v <= hi {
                1.0
            } else if fall <= 0.0 {
                0.0
            } else {
                let d = if *v < lo { lo - v } else { v - hi };
                let t = (1.0 - d / fall).max(0.0);
                t * t * (3.0 - 2.0 * t)
            };
            if set.invert { 1.0 - m } else { m }
        })
        .collect();
    blur(&mut mask, &g, set.blur);
    let mut out = input.clone();
    if let Some(prev) = input.point_attributes.get("mask").filter(|m| m.len() == mask.len()) {
        for (m, p) in mask.iter_mut().zip(prev) {
            *m = match set.combine {
                1 => *m * p,
                2 => (*m + p).min(1.0),
                3 => m.max(*p),
                4 => m.min(*p),
                _ => *m,
            };
        }
    }
    if set.visualize {
        out.point_attributes.insert("Cd".into(), mask.iter().flat_map(|m| mask_colour(*m)).collect());
    }
    out.point_attributes.insert("mask".into(), mask);
    Ok(out)
}

// --- Colorize ----------------------------------------------------------------

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

// 0 below `edge - width`, 1 above `edge + width`, smooth between.
fn step(edge: f32, width: f32, v: f32) -> f32 {
    let w = width.max(1e-4);
    let t = ((v - edge + w) / (2.0 * w)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn terrain_colorize(input: &Mesh, set: &ColorizeSettings) -> Result<Mesh, String> {
    let g = grid_of(input)?;
    let h = heights(input);
    let slope = slopes(&h, &g);
    let lo = h.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = h.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(1e-6);
    let b = set.blend.max(0.0);
    let colours: Vec<f32> = h
        .iter()
        .zip(&slope)
        .flat_map(|(h, s)| {
            let t = (h - lo) / span;
            let mut c = mix(set.sand, set.grass, step(set.sand_level, b, t));
            c = mix(c, set.rock, step(set.rock_slope, b * 100.0, *s));
            // Snow settles on the gentler slopes only.
            let snow = step(set.snow_level, b, t) * (1.0 - step(set.rock_slope + 15.0, b * 100.0, *s));
            mix(c, set.snow, snow)
        })
        .collect();
    let mut out = input.clone();
    out.point_attributes.insert("Cd".into(), colours);
    Ok(out)
}

#[allow(dead_code)]
pub fn point(mesh: &Mesh, x: usize, z: usize) -> Option<Vec3> {
    let g = grid_of(mesh).ok()?;
    (x < g.nx && z < g.nz).then(|| mesh.vertices[z * g.nx + x])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Mesh {
        terrain(&TerrainSettings { size: [10.0, 6.0], resolution: 100 })
    }

    #[test]
    fn terrain_grid_shape() {
        let t = base();
        let g = grid_of(&t).unwrap();
        assert_eq!((g.nx, g.nz), (101, 61));
        assert!((g.dx - 0.1).abs() < 1e-5 && (g.dz - 0.1).abs() < 1e-5);
        assert_eq!(point(&t, 100, 60).unwrap(), Vec3::new(5.0, 0.0, 3.0));
        assert!(terrain_noise(&super::super::node_grid(1.0, 1.0, 3, 3), &NoiseSettings::defaults()).is_err());
    }

    #[test]
    fn noise_types_make_varied_deterministic_heights() {
        let t = base();
        for kind in 0..NOISE_TYPES.len() {
            let set = NoiseSettings { kind, ..NoiseSettings::defaults() };
            let a = terrain_noise(&t, &set).unwrap();
            let h = heights(&a);
            let (lo, hi) = (h.iter().copied().fold(f32::MAX, f32::min), h.iter().copied().fold(f32::MIN, f32::max));
            assert!(lo.abs() < 1e-4 && (hi - 1.5).abs() < 1e-3, "{}: {lo}..{hi}", NOISE_TYPES[kind]);
            assert_eq!(h, heights(&terrain_noise(&t, &set).unwrap()));
            let other = terrain_noise(&t, &NoiseSettings { seed: 7, ..set }).unwrap();
            assert_ne!(h, heights(&other));
        }
    }

    #[test]
    fn erosion_carves_and_keeps_the_grid() {
        let hills = terrain_noise(&base(), &NoiseSettings { kind: 1, ..NoiseSettings::defaults() }).unwrap();
        let set = ErodeSettings { droplets: 20_000, ..ErodeSettings::defaults() };
        let out = terrain_erode(&hills, &set).unwrap();
        assert_eq!(out.vertices.len(), hills.vertices.len());
        assert!(out.vertices.iter().all(|v| v.y.is_finite()));
        let changed = out.vertices.iter().zip(&hills.vertices).filter(|(a, b)| (a.y - b.y).abs() > 1e-4).count();
        assert!(changed > hills.vertices.len() / 10, "only {changed} points changed");
        // Erosion moves material, it doesn't make much of it.
        let sum = |m: &Mesh| m.vertices.iter().map(|v| v.y as f64).sum::<f64>();
        assert!((sum(&out) - sum(&hills)).abs() / sum(&hills) < 0.1, "{} vs {}", sum(&out), sum(&hills));
        // Slopes end up gentler: thermal erosion alone flattens a spike.
        let mut spike = base();
        spike.vertices[30 * 101 + 50].y = 5.0;
        let flat = terrain_erode(&spike, &ErodeSettings { droplets: 0, thermal_iterations: 200, ..ErodeSettings::defaults() }).unwrap();
        assert!(flat.vertices[30 * 101 + 50].y < 2.0);
        let flow = &out.point_attributes["flow"];
        assert!(flow.iter().all(|f| (0.0..=1.0).contains(f)) && flow.iter().any(|f| *f > 0.5));
    }

    #[test]
    fn terrace_makes_flat_steps() {
        let mut ramp = base();
        for v in &mut ramp.vertices {
            v.y = v.x + 5.0;
        }
        let out = terrain_terrace(&ramp, &TerraceSettings { step: 1.0, offset: 0.0, sharpness: 1.0, use_mask: false }).unwrap();
        assert!(out.vertices.iter().all(|v| (v.y - v.y.round()).abs() < 1e-4 || (v.y - v.y.floor()).abs() < 1e-4));
        assert!(out.vertices.iter().all(|v| v.y <= ramp.vertices[0].y.max(v.y) && v.y.fract().abs() < 1e-4));
    }

    #[test]
    fn masks_drive_noise_and_colour() {
        let mut ramp = base();
        for v in &mut ramp.vertices {
            v.y = v.x;
        }
        let masked = terrain_mask(&ramp, &MaskSettings { source: 0, min: 2.0, max: 10.0, falloff: 0.0, blur: 0, invert: false, combine: 0, visualize: true }).unwrap();
        let m = &masked.point_attributes["mask"];
        for (v, m) in masked.vertices.iter().zip(m) {
            assert_eq!(*m, if v.y >= 2.0 { 1.0 } else { 0.0 });
        }
        assert_eq!(masked.point_attributes["Cd"].len(), m.len() * 3);
        let slope = terrain_mask(&ramp, &MaskSettings { source: 1, min: 40.0, max: 50.0, falloff: 0.0, blur: 0, invert: false, combine: 0, visualize: false }).unwrap();
        assert!(slope.point_attributes["mask"].iter().all(|m| *m == 1.0), "45° everywhere");
        // Noise only where the mask is on.
        let noisy = terrain_noise(&masked, &NoiseSettings { use_mask: true, ..NoiseSettings::defaults() }).unwrap();
        for ((a, b), m) in noisy.vertices.iter().zip(&masked.vertices).zip(m) {
            if *m == 0.0 {
                assert_eq!(a.y, b.y);
            }
        }
        assert!(terrain_mask(&ramp, &MaskSettings { source: 2, min: 0.0, max: 1.0, falloff: 0.0, blur: 0, invert: false, combine: 0, visualize: false }).is_err());
        let coloured = terrain_colorize(&noisy, &ColorizeSettings::defaults()).unwrap();
        assert_eq!(coloured.point_attributes["Cd"].len(), coloured.vertices.len() * 3);
    }
}
