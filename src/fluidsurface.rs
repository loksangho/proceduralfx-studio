// Particle Fluid Surface: particles (e.g. FLIP) to a smooth, closed water surface.
//
// Each voxel takes the weighted average position x̄ of the particles near it (Zhu &
// Bridson, "Animating Sand as a Fluid"), and its signed distance is |x − x̄| − r. A
// lone particle becomes a sphere of the particle radius; a body of particles becomes
// one surface, flat where the particles are, with no bumps from single particles.
// The level set is then smoothed and meshed (surface nets), and the particles'
// point attributes (v, Cd, ...) are carried onto the surface's points.

use super::volume::{self, Class, Volume};
use super::{Mesh, Vec3};

// Dense grids larger than this are made coarser, to keep memory and time in check.
const MAX_VOXELS: usize = 12_000_000;

pub struct SurfaceSettings {
    // A lone particle's radius.
    pub radius: f32,
    // Extra reach: particles further apart than this still join into one surface.
    pub join_distance: f32,
    // Voxels across the particles' longest side (finer than a quarter radius isn't used).
    pub resolution: usize,
    // Smoothing passes over the level set.
    pub smoothing: usize,
}

// Particles binned in cells of the kernel's size: each cell's particle indices, contiguous.
struct Bins {
    origin: Vec3,
    size: f32,
    dims: [usize; 3],
    start: Vec<u32>,
    items: Vec<u32>,
}

impl Bins {
    fn new(points: &[Vec3], origin: Vec3, size: f32, extent: Vec3) -> Bins {
        let dims = [extent.x, extent.y, extent.z].map(|e| ((e / size).ceil() as usize).max(1));
        let index = |p: Vec3| {
            let c = [(p.x - origin.x) / size, (p.y - origin.y) / size, (p.z - origin.z) / size];
            let c: [usize; 3] = std::array::from_fn(|a| (c[a].max(0.0) as usize).min(dims[a] - 1));
            c[0] + dims[0] * (c[1] + dims[1] * c[2])
        };
        let cells = dims[0] * dims[1] * dims[2];
        let mut start = vec![0u32; cells + 1];
        let homes: Vec<usize> = points.iter().map(|p| index(*p)).collect();
        for &h in &homes {
            start[h + 1] += 1;
        }
        for i in 0..cells {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut items = vec![0u32; points.len()];
        for (i, &h) in homes.iter().enumerate() {
            items[fill[h] as usize] = i as u32;
            fill[h] += 1;
        }
        Bins { origin, size, dims, start, items }
    }

    // Every particle in the cells around p (a superset of those within one cell size).
    fn near(&self, p: Vec3, mut visit: impl FnMut(usize)) {
        let c = [(p.x - self.origin.x) / self.size, (p.y - self.origin.y) / self.size, (p.z - self.origin.z) / self.size].map(|v| v.floor() as i64);
        for z in c[2] - 1..=c[2] + 1 {
            for y in c[1] - 1..=c[1] + 1 {
                for x in c[0] - 1..=c[0] + 1 {
                    if x < 0 || y < 0 || z < 0 || x >= self.dims[0] as i64 || y >= self.dims[1] as i64 || z >= self.dims[2] as i64 {
                        continue;
                    }
                    let cell = x as usize + self.dims[0] * (y as usize + self.dims[1] * z as usize);
                    for &i in &self.items[self.start[cell] as usize..self.start[cell + 1] as usize] {
                        visit(i as usize);
                    }
                }
            }
        }
    }
}

// Smooth falloff: 1 at the particle, 0 at the kernel's edge and beyond.
fn kernel(d2: f32, reach2: f32) -> f32 {
    if d2 >= reach2 {
        0.0
    } else {
        let s = 1.0 - d2 / reach2;
        s * s * s
    }
}

pub fn surface(input: &Mesh, set: &SurfaceSettings) -> Mesh {
    let points = &input.vertices;
    let Some((lo, hi)) = input.bounds() else {
        return Mesh::default();
    };
    if points.is_empty() {
        return Mesh::default();
    }
    let radius = set.radius.max(1e-4);
    let reach = 2.0 * radius + set.join_distance.max(0.0);
    let reach2 = reach * reach;
    // The grid covers the particles and a kernel's reach around them.
    let pad = Vec3::new(reach, reach, reach);
    let (lo, hi) = (lo.sub(pad), hi.add(pad));
    let extent = hi.sub(lo);
    let longest = extent.x.max(extent.y).max(extent.z);
    let mut voxel = (longest / set.resolution.max(8) as f32).max(radius * 0.25);
    let dims = loop {
        let dims = [extent.x, extent.y, extent.z].map(|e| (e / voxel).ceil() as usize + 1);
        if dims[0] * dims[1] * dims[2] <= MAX_VOXELS {
            break dims;
        }
        voxel *= 1.1;
    };
    let bins = Bins::new(points, lo, reach, extent);
    let background = reach;

    // Signed distances, a slab of z-slices per thread.
    let mut values = vec![background; dims[0] * dims[1] * dims[2]];
    let slice = dims[0] * dims[1];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(dims[2]);
    let slabs = dims[2].div_ceil(threads);
    std::thread::scope(|scope| {
        for (t, chunk) in values.chunks_mut(slabs * slice).enumerate() {
            let bins = &bins;
            scope.spawn(move || {
                for (k, value) in chunk.iter_mut().enumerate() {
                    let (x, y, z) = (k % dims[0], (k / dims[0]) % dims[1], t * slabs + k / slice);
                    let at = Vec3::new(lo.x + x as f32 * voxel, lo.y + y as f32 * voxel, lo.z + z as f32 * voxel);
                    let (mut weight, mut sum) = (0.0f32, Vec3::zero());
                    bins.near(at, |i| {
                        let p = points[i];
                        let w = kernel(at.sub(p).dot(at.sub(p)), reach2);
                        if w > 0.0 {
                            weight += w;
                            sum = sum.add(p.scale(w));
                        }
                    });
                    if weight > 0.0 {
                        let centre = sum.scale(1.0 / weight);
                        *value = (at.sub(centre).length() - radius).clamp(-background, background);
                    }
                }
            });
        }
    });
    let mut level_set = Volume { class: Class::LevelSet, voxel, origin: lo, dims, values, background };
    volume::smooth(&mut level_set, set.smoothing);
    let mut mesh = volume::to_polygons(&level_set, 0.0);
    carry_attributes(input, &bins, reach2, &mut mesh);
    mesh
}

// The particles' own point attributes (v, Cd, ...), averaged onto the surface points
// with the same falloff, each from the particles nearest it.
fn carry_attributes(input: &Mesh, bins: &Bins, reach2: f32, mesh: &mut Mesh) {
    let n = input.vertices.len();
    let carried: Vec<(&String, usize)> = input
        .point_attributes
        .iter()
        .filter(|(name, values)| !name.contains(':') && !values.is_empty() && values.len() % n == 0)
        .map(|(name, values)| (name, values.len() / n))
        .collect();
    if carried.is_empty() {
        return;
    }
    let mut out: Vec<Vec<f32>> = carried.iter().map(|(_, k)| Vec::with_capacity(mesh.vertices.len() * k)).collect();
    for &at in &mesh.vertices {
        let (mut weight, mut nearest) = (0.0f32, (f32::INFINITY, 0usize));
        let mut sums: Vec<Vec<f32>> = carried.iter().map(|(_, k)| vec![0.0; *k]).collect();
        bins.near(at, |i| {
            let d2 = at.sub(input.vertices[i]).dot(at.sub(input.vertices[i]));
            if d2 < nearest.0 {
                nearest = (d2, i);
            }
            let w = kernel(d2, reach2);
            if w > 0.0 {
                weight += w;
                for ((name, k), sum) in carried.iter().zip(&mut sums) {
                    let values = &input.point_attributes[*name];
                    for c in 0..*k {
                        sum[c] += values[i * k + c] * w;
                    }
                }
            }
        });
        for (((name, k), sum), out) in carried.iter().zip(&sums).zip(&mut out) {
            if weight > 0.0 {
                out.extend(sum.iter().map(|s| s / weight));
            } else {
                let values = &input.point_attributes[*name];
                let i = nearest.1;
                out.extend_from_slice(&values[i * k..i * k + k]);
            }
        }
    }
    for ((name, _), values) in carried.into_iter().zip(out) {
        mesh.point_attributes.insert(name.clone(), values);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(radius: f32, resolution: usize) -> SurfaceSettings {
        SurfaceSettings { radius, join_distance: 0.0, resolution, smoothing: 0 }
    }

    fn signed_volume(mesh: &Mesh) -> f32 {
        mesh.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            a.dot(b.cross(c))
        }).sum::<f32>() / 6.0
    }

    #[test]
    fn a_lone_particle_is_a_round_drop() {
        let drop = Mesh { vertices: vec![Vec3::new(1.0, 2.0, 3.0)], ..Default::default() };
        let mesh = surface(&drop, &settings(0.5, 64));
        assert!(mesh.indices.len() > 100);
        for v in &mesh.vertices {
            let r = v.sub(Vec3::new(1.0, 2.0, 3.0)).length();
            assert!((r - 0.5).abs() < 0.03, "vertex at {r} from the centre, not on the 0.5 sphere");
        }
        // Closed and wound like the app's primitives (as VDB's level sets are).
        let reference = signed_volume(&crate::node_sphere(0.5, 16, 32));
        let volume = signed_volume(&mesh);
        assert!(volume.signum() == reference.signum() && (volume.abs() - reference.abs()).abs() < 0.1 * reference.abs(), "{volume} vs {reference}");
    }

    #[test]
    fn a_block_of_particles_has_a_flat_top_and_no_voxel_steps() {
        // A 2 × 0.5 × 2 slab of water, jittered like FLIP particles.
        let mut points = Vec::new();
        let mut seed = 1u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        for z in 0..40 {
            for y in 0..10 {
                for x in 0..40 {
                    let j = 0.05 * 0.6;
                    points.push(Vec3::new(x as f32 * 0.05 + rand() * j, y as f32 * 0.05 + rand() * j, z as f32 * 0.05 + rand() * j));
                }
            }
        }
        let water = Mesh { vertices: points, ..Default::default() };
        let mesh = surface(&water, &SurfaceSettings { radius: 0.05, join_distance: 0.0, resolution: 120, smoothing: 1 });
        // Points on the top, away from the edges, sit on one plane.
        let top: Vec<f32> = mesh.vertices.iter().filter(|v| v.y > 0.3 && (0.3..1.6).contains(&v.x) && (0.3..1.6).contains(&v.z)).map(|v| v.y).collect();
        assert!(top.len() > 100);
        let mean = top.iter().sum::<f32>() / top.len() as f32;
        // It follows the jittered top particles (±0.015), with no voxel steps on top.
        let rms = (top.iter().map(|y| (y - mean).powi(2)).sum::<f32>() / top.len() as f32).sqrt();
        assert!(rms < 0.008, "the top surface varies by {rms} (rms)");
        assert!((mean - 0.45 - 0.05).abs() < 0.03, "top at {mean}");
        // One piece: no stray droplets around a dense body.
        let volume = signed_volume(&mesh).abs();
        assert!((volume - 2.0 * 2.0 * 0.5).abs() < 0.6, "volume {volume}");
    }

    #[test]
    fn particle_velocities_reach_the_surface() {
        let points = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0)];
        let mut drops = Mesh { vertices: points, ..Default::default() };
        drops.point_attributes.insert("v".into(), vec![1.0, 0.0, 0.0, 0.0, -2.0, 0.0]);
        let mesh = surface(&drops, &settings(0.3, 100));
        let v = &mesh.point_attributes["v"];
        assert_eq!(v.len(), mesh.vertices.len() * 3);
        for (i, p) in mesh.vertices.iter().enumerate() {
            let expected = if p.x < 2.5 { [1.0, 0.0, 0.0] } else { [0.0, -2.0, 0.0] };
            assert_eq!([v[i * 3], v[i * 3 + 1], v[i * 3 + 2]], expected);
        }
    }
}
