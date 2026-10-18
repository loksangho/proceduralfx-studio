// VDB-style voxel volumes for the VDB and Convert VDB nodes.
//
// Volumes travel through the (mesh-based) network as point geometry: one point
// per active voxel at its centre, carrying the grid value (`surface` for
// signed-distance level sets, `density` for fog volumes), with detail
// attributes describing the grid. They also carry Alpha, Cd and pscale so the
// viewport draws them as soft volumes, like Pyro output.
//
//   detail:volume_voxelsize   voxel edge length
//   detail:volume_origin      world centre of voxel index (0, 0, 0)
//   detail:volume_class       0 level set (SDF), 1 fog
//   detail:volume_background  value outside the active voxels (level sets: half band width)

use super::{Mesh, Vec3};
use std::collections::{HashMap, VecDeque};

// Dense grids larger than this are refused instead of exhausting memory.
const MAX_VOXELS: usize = 24_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Class {
    LevelSet,
    Fog,
}

#[derive(Clone, Debug)]
pub struct Volume {
    pub class: Class,
    pub voxel: f32,
    // World centre of dense index (0, 0, 0).
    pub origin: Vec3,
    pub dims: [usize; 3],
    pub values: Vec<f32>,
    pub background: f32,
}

impl Volume {
    pub fn name(&self) -> &'static str {
        match self.class {
            Class::LevelSet => "surface",
            Class::Fog => "density",
        }
    }

    fn at(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.dims[0] * (y + self.dims[1] * z)
    }

    fn center(&self, x: usize, y: usize, z: usize) -> Vec3 {
        Vec3::new(
            self.origin.x + x as f32 * self.voxel,
            self.origin.y + y as f32 * self.voxel,
            self.origin.z + z as f32 * self.voxel,
        )
    }

    fn get(&self, x: usize, y: usize, z: usize) -> f32 {
        self.values[self.at(x, y, z)]
    }

    // Trilinear value at a world position; outside the grid reads the background.
    pub fn sample(&self, p: Vec3) -> f32 {
        let g = [(p.x - self.origin.x) / self.voxel, (p.y - self.origin.y) / self.voxel, (p.z - self.origin.z) / self.voxel];
        if (0..3).any(|a| !(g[a] >= 0.0 && g[a] <= (self.dims[a] - 1) as f32)) {
            return self.background;
        }
        let mut base = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for a in 0..3 {
            let i = (g[a].floor() as usize).min(self.dims[a].saturating_sub(2));
            base[a] = i;
            frac[a] = g[a] - i as f32;
        }
        let [x, y, z] = base;
        let [fx, fy, fz] = frac;
        let v = |dx: usize, dy: usize, dz: usize| self.get((x + dx).min(self.dims[0] - 1), (y + dy).min(self.dims[1] - 1), (z + dz).min(self.dims[2] - 1));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        lerp(
            lerp(lerp(v(0, 0, 0), v(1, 0, 0), fx), lerp(v(0, 1, 0), v(1, 1, 0), fx), fy),
            lerp(lerp(v(0, 0, 1), v(1, 0, 1), fx), lerp(v(0, 1, 1), v(1, 1, 1), fx), fy),
            fz,
        )
    }

    // Central-difference gradient (unnormalised) at a world position.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        let h = self.voxel * 0.5;
        let d = |o: Vec3| self.sample(p.add(o)) - self.sample(p.sub(o));
        Vec3::new(d(Vec3::new(h, 0.0, 0.0)), d(Vec3::new(0.0, h, 0.0)), d(Vec3::new(0.0, 0.0, h))).scale(1.0 / (2.0 * h))
    }

    pub fn active_count(&self) -> usize {
        self.values.iter().filter(|v| self.is_active(**v)).count()
    }

    fn is_active(&self, v: f32) -> bool {
        match self.class {
            Class::LevelSet => v.abs() < self.background,
            Class::Fog => v > 1e-4,
        }
    }
}

pub fn is_volume(mesh: &Mesh) -> bool {
    mesh.point_attributes.contains_key("detail:volume_voxelsize")
}

fn detail(mesh: &Mesh, name: &str) -> Option<Vec<f32>> {
    mesh.point_attributes.get(&format!("detail:{name}")).cloned()
}

// Rebuilds the dense grid from voxel points. Level-set voxels outside the band
// are filled with ±background by flood fill: cells connected to the grid
// boundary without crossing the band are outside.
pub fn from_mesh(mesh: &Mesh) -> Result<Volume, String> {
    let voxel = detail(mesh, "volume_voxelsize").and_then(|v| v.first().copied()).filter(|v| *v > 0.0).ok_or("the input is not a VDB or volume")?;
    let origin = detail(mesh, "volume_origin").filter(|o| o.len() >= 3).map_or(Vec3::zero(), |o| Vec3::new(o[0], o[1], o[2]));
    let class = if detail(mesh, "volume_class").and_then(|c| c.first().copied()).unwrap_or(0.0) >= 0.5 { Class::Fog } else { Class::LevelSet };
    let name = match class {
        Class::LevelSet => "surface",
        Class::Fog => "density",
    };
    let values = mesh.point_attributes.get(name).filter(|v| v.len() == mesh.vertices.len()).ok_or_else(|| format!("the volume has no {name} values"))?;
    let background = detail(mesh, "volume_background").and_then(|b| b.first().copied()).unwrap_or(match class {
        Class::LevelSet => voxel * 3.0,
        Class::Fog => 0.0,
    });
    if mesh.vertices.is_empty() {
        return Err("the volume is empty".into());
    }
    let cells: Vec<[i64; 3]> = mesh
        .vertices
        .iter()
        .map(|p| [((p.x - origin.x) / voxel).round() as i64, ((p.y - origin.y) / voxel).round() as i64, ((p.z - origin.z) / voxel).round() as i64])
        .collect();
    // One empty cell of padding on every side, so the flood fill can go around the band.
    let mut lo = [i64::MAX; 3];
    let mut hi = [i64::MIN; 3];
    for c in &cells {
        for a in 0..3 {
            lo[a] = lo[a].min(c[a] - 1);
            hi[a] = hi[a].max(c[a] + 1);
        }
    }
    let dims = [0, 1, 2].map(|a| (hi[a] - lo[a] + 1) as usize);
    if dims.iter().product::<usize>() > MAX_VOXELS {
        return Err(format!("the volume is too large ({}×{}×{} voxels)", dims[0], dims[1], dims[2]));
    }
    let mut volume = Volume {
        class,
        voxel,
        origin: Vec3::new(origin.x + lo[0] as f32 * voxel, origin.y + lo[1] as f32 * voxel, origin.z + lo[2] as f32 * voxel),
        dims,
        values: vec![f32::NAN; dims.iter().product()],
        background,
    };
    for (c, value) in cells.iter().zip(values) {
        let i = volume.at((c[0] - lo[0]) as usize, (c[1] - lo[1]) as usize, (c[2] - lo[2]) as usize);
        volume.values[i] = *value;
    }
    match class {
        Class::Fog => volume.values.iter_mut().filter(|v| v.is_nan()).for_each(|v| *v = 0.0),
        Class::LevelSet => flood_fill_sign(&mut volume),
    }
    Ok(volume)
}

fn flood_fill_sign(volume: &mut Volume) {
    let [nx, ny, nz] = volume.dims;
    let mut queue = VecDeque::new();
    let mut outside = vec![false; volume.values.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let border = x == 0 || y == 0 || z == 0 || x == nx - 1 || y == ny - 1 || z == nz - 1;
                let i = volume.at(x, y, z);
                if border && volume.values[i].is_nan() {
                    outside[i] = true;
                    queue.push_back((x, y, z));
                }
            }
        }
    }
    while let Some((x, y, z)) = queue.pop_front() {
        let neighbours = [
            (x.wrapping_sub(1), y, z),
            (x + 1, y, z),
            (x, y.wrapping_sub(1), z),
            (x, y + 1, z),
            (x, y, z.wrapping_sub(1)),
            (x, y, z + 1),
        ];
        for (a, b, c) in neighbours {
            if a < nx && b < ny && c < nz {
                let i = volume.at(a, b, c);
                if !outside[i] && volume.values[i].is_nan() {
                    outside[i] = true;
                    queue.push_back((a, b, c));
                }
            }
        }
    }
    let background = volume.background;
    for (value, out) in volume.values.iter_mut().zip(outside) {
        if value.is_nan() {
            *value = if out { background } else { -background };
        }
    }
}

// Voxel points for the network, with display attributes for the viewport.
pub fn to_mesh(volume: &Volume) -> Mesh {
    let [nx, ny, nz] = volume.dims;
    let mut voxels = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let v = volume.get(x, y, z);
                if volume.is_active(v) {
                    voxels.push(([x as i64, y as i64, z as i64], v));
                }
            }
        }
    }
    encode_voxels(&voxels, volume.class, volume.voxel, volume.origin, volume.background)
}

// Encodes sparse voxels (index, value); world position = origin + index · voxel.
pub fn encode_voxels(voxels: &[([i64; 3], f32)], class: Class, voxel: f32, origin: Vec3, background: f32) -> Mesh {
    let mut mesh = Mesh::default();
    let (mut values, mut alpha, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    for (index, v) in voxels {
        mesh.vertices.push(Vec3::new(
            origin.x + index[0] as f32 * voxel,
            origin.y + index[1] as f32 * voxel,
            origin.z + index[2] as f32 * voxel,
        ));
        values.push(*v);
        let (a, c) = match class {
            // Level sets show a translucent shell around the surface.
            Class::LevelSet => ((0.35 * (1.0 - v.abs() / voxel)).clamp(0.0, 0.35), [0.62, 0.72, 0.85]),
            Class::Fog => ((1.0 - (-v * 1.5).exp()).clamp(0.0, 0.9), [0.8, 0.8, 0.82]),
        };
        alpha.push(a);
        colors.extend(c);
    }
    let n = mesh.vertices.len();
    let name = match class {
        Class::LevelSet => "surface",
        Class::Fog => "density",
    };
    mesh.point_attributes.insert(name.into(), values);
    mesh.point_attributes.insert("Alpha".into(), alpha);
    mesh.point_attributes.insert("Cd".into(), colors);
    mesh.point_attributes.insert("pscale".into(), vec![voxel * 0.85; n]);
    mesh.point_attributes.insert("detail:volume_voxelsize".into(), vec![voxel]);
    mesh.point_attributes.insert("detail:volume_origin".into(), vec![origin.x, origin.y, origin.z]);
    mesh.point_attributes.insert("detail:volume_class".into(), vec![if class == Class::Fog { 1.0 } else { 0.0 }]);
    mesh.point_attributes.insert("detail:volume_background".into(), vec![background]);
    mesh
}

// The sparse voxels of an encoded volume: (index, value), class, voxel size, origin, background.
pub fn decode_voxels(mesh: &Mesh) -> Result<(Vec<([i64; 3], f32)>, Class, f32, Vec3, f32), String> {
    let voxel = detail(mesh, "volume_voxelsize").and_then(|v| v.first().copied()).filter(|v| *v > 0.0).ok_or("the input is not a VDB or volume")?;
    let origin = detail(mesh, "volume_origin").filter(|o| o.len() >= 3).map_or(Vec3::zero(), |o| Vec3::new(o[0], o[1], o[2]));
    let class = if detail(mesh, "volume_class").and_then(|c| c.first().copied()).unwrap_or(0.0) >= 0.5 { Class::Fog } else { Class::LevelSet };
    let name = if class == Class::Fog { "density" } else { "surface" };
    let values = mesh.point_attributes.get(name).filter(|v| v.len() == mesh.vertices.len()).ok_or_else(|| format!("the volume has no {name} values"))?;
    let background = detail(mesh, "volume_background").and_then(|b| b.first().copied()).unwrap_or(if class == Class::Fog { 0.0 } else { voxel * 3.0 });
    let voxels = mesh
        .vertices
        .iter()
        .zip(values)
        .map(|(p, v)| ([((p.x - origin.x) / voxel).round() as i64, ((p.y - origin.y) / voxel).round() as i64, ((p.z - origin.z) / voxel).round() as i64], *v))
        .collect();
    Ok((voxels, class, voxel, origin, background))
}

pub(crate) fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Ericson, Real-Time Collision Detection 5.1.5.
    let (ab, ac, ap) = (b.sub(a), c.sub(a), p.sub(a));
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p.sub(b);
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a.add(ab.scale(d1 / (d1 - d3)));
    }
    let cp = p.sub(c);
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a.add(ac.scale(d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b.add(c.sub(b).scale((d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    a.add(ab.scale(vb * denom)).add(ac.scale(vc * denom))
}

// Signed distance level set of a closed mesh: distances within `band` voxels of the
// surface, signed negative inside (by ray parity), ±background elsewhere.
pub fn from_polygons(mesh: &Mesh, voxel: f32, band: f32) -> Result<Volume, String> {
    let voxel = voxel.max(1e-4);
    let (lo, hi) = mesh.bounds().filter(|_| !mesh.indices.is_empty()).ok_or("the input has no polygons")?;
    let margin = band + 1.0;
    let start = Vec3::new(
        ((lo.x / voxel).floor() - margin).round() * voxel,
        ((lo.y / voxel).floor() - margin).round() * voxel,
        ((lo.z / voxel).floor() - margin).round() * voxel,
    );
    let dims = [hi.x - start.x, hi.y - start.y, hi.z - start.z].map(|extent| (extent / voxel + margin).ceil() as usize + 1);
    if dims.iter().product::<usize>() > MAX_VOXELS {
        return Err(format!("the voxel size is too small for this mesh ({}×{}×{} voxels)", dims[0], dims[1], dims[2]));
    }
    let background = band * voxel;
    let mut volume = Volume { class: Class::LevelSet, voxel, origin: start, dims, values: vec![background; dims.iter().product()], background };
    let cell = |v: f32, o: f32| ((v - o) / voxel).round();
    for tri in &mesh.indices {
        let [a, b, c] = tri.map(|i| mesh.vertices[i]);
        let range = |axis: fn(Vec3) -> f32, o: f32, n: usize| {
            let min = cell(axis(a).min(axis(b)).min(axis(c)), o) - band;
            let max = cell(axis(a).max(axis(b)).max(axis(c)), o) + band;
            (min.max(0.0) as usize)..((max + 1.0).max(0.0) as usize).min(n)
        };
        for z in range(|v| v.z, start.z, dims[2]) {
            for y in range(|v| v.y, start.y, dims[1]) {
                for x in range(|v| v.x, start.x, dims[0]) {
                    let p = volume.center(x, y, z);
                    let d = closest_on_triangle(p, a, b, c).sub(p).length();
                    let i = volume.at(x, y, z);
                    volume.values[i] = volume.values[i].min(d);
                }
            }
        }
    }
    // Sign from scanline parity along X (rays nudged off-centre to avoid shared edges).
    let [nx, ny, nz] = dims;
    let mut rows: Vec<Vec<f32>> = vec![Vec::new(); ny * nz];
    for tri in &mesh.indices {
        let [a, b, c] = tri.map(|i| mesh.vertices[i]);
        let d = (b.y - a.y) * (c.z - a.z) - (c.y - a.y) * (b.z - a.z);
        if d.abs() < 1e-12 {
            continue;
        }
        let yr = (cell(a.y.min(b.y).min(c.y), start.y).max(0.0) as usize)..((cell(a.y.max(b.y).max(c.y), start.y) + 1.0).max(0.0) as usize).min(ny);
        let zr = (cell(a.z.min(b.z).min(c.z), start.z).max(0.0) as usize)..((cell(a.z.max(b.z).max(c.z), start.z) + 1.0).max(0.0) as usize).min(nz);
        for z in zr {
            for y in yr.clone() {
                let centre = volume.center(0, y, z);
                let p = Vec3::new(centre.x, centre.y + voxel * 1.234e-3, centre.z + voxel * 2.718e-3);
                let u = ((b.y - p.y) * (c.z - p.z) - (c.y - p.y) * (b.z - p.z)) / d;
                let v = ((c.y - p.y) * (a.z - p.z) - (a.y - p.y) * (c.z - p.z)) / d;
                let w = 1.0 - u - v;
                if u >= 0.0 && v >= 0.0 && w >= 0.0 {
                    rows[y + ny * z].push(u * a.x + v * b.x + w * c.x);
                }
            }
        }
    }
    for z in 0..nz {
        for y in 0..ny {
            let row = &mut rows[y + ny * z];
            row.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
            for pair in row.chunks_exact(2) {
                for x in 0..nx {
                    let px = start.x + x as f32 * voxel;
                    if px >= pair[0] && px <= pair[1] {
                        let i = volume.at(x, y, z);
                        volume.values[i] = -volume.values[i];
                    }
                }
            }
        }
    }
    Ok(volume)
}

// Shifts the surface outward (positive) or inward by `distance` world units.
// Narrow-band level sets only know distances near the surface; this fills in
// approximate distances everywhere (a two-pass 3-4-5 style chamfer transform),
// keeping each voxel's inside/outside sign, so a point that ends up deep inside
// a collider still sees a gradient pointing back out.
pub fn fill_distances(volume: &mut Volume) {
    let [nx, ny, nz] = volume.dims;
    if nx * ny * nz == 0 {
        return;
    }
    let known = volume.background * 0.999;
    let mut d: Vec<f32> = volume.values.iter().map(|v| if v.abs() < known { v.abs() } else { f32::INFINITY }).collect();
    let h = volume.voxel;
    let (d1, d2, d3) = (h, h * std::f32::consts::SQRT_2, h * 3f32.sqrt());
    let at = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    // Neighbours visited before the current voxel in the forward scan order.
    let mut back: Vec<(i32, i32, i32, f32)> = Vec::new();
    for dz in -1i32..=1 {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let earlier = dz < 0 || (dz == 0 && (dy < 0 || (dy == 0 && dx < 0)));
                if earlier {
                    let n = dx.abs() + dy.abs() + dz.abs();
                    back.push((dx, dy, dz, [0.0, d1, d2, d3][n as usize]));
                }
            }
        }
    }
    for pass in 0..2 {
        let order: Box<dyn Iterator<Item = usize>> = if pass == 0 { Box::new(0..nx * ny * nz) } else { Box::new((0..nx * ny * nz).rev()) };
        let sign = if pass == 0 { 1 } else { -1 };
        for i in order {
            let (x, y, z) = (i % nx, (i / nx) % ny, i / (nx * ny));
            let mut best = d[i];
            for &(dx, dy, dz, w) in &back {
                let (qx, qy, qz) = (x as i32 + dx * sign, y as i32 + dy * sign, z as i32 + dz * sign);
                if qx < 0 || qy < 0 || qz < 0 || qx >= nx as i32 || qy >= ny as i32 || qz >= nz as i32 {
                    continue;
                }
                best = best.min(d[at(qx as usize, qy as usize, qz as usize)] + w);
            }
            d[i] = best;
        }
    }
    for (v, dist) in volume.values.iter_mut().zip(d) {
        if dist.is_finite() {
            *v = if *v < 0.0 { -dist } else { dist };
        }
    }
    volume.background = volume.values.iter().fold(volume.background, |m, v| m.max(v.abs()));
}

pub fn offset(volume: &mut Volume, distance: f32) {
    if volume.class == Class::LevelSet {
        for v in &mut volume.values {
            *v = (*v - distance).clamp(-volume.background, volume.background);
        }
    }
}

// Box-blurs the values `passes` times (smooths level-set surfaces).
pub fn smooth(volume: &mut Volume, passes: usize) {
    let [nx, ny, nz] = volume.dims;
    for _ in 0..passes {
        let source = volume.values.clone();
        for z in 1..nz.saturating_sub(1) {
            for y in 1..ny.saturating_sub(1) {
                for x in 1..nx.saturating_sub(1) {
                    let mut sum = 0.0;
                    for (dx, dy, dz) in [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1)] {
                        sum += source[volume.at(x + dx, y + dy, z + dz)] + source[volume.at(x - dx, y - dy, z - dz)];
                    }
                    let i = volume.at(x, y, z);
                    volume.values[i] = sum / 8.0;
                }
            }
        }
    }
}

// Level set to fog: 1 inside, a one-voxel ramp across the surface, 0 outside.
pub fn to_fog(volume: &Volume) -> Volume {
    match volume.class {
        Class::Fog => volume.clone(),
        Class::LevelSet => Volume {
            class: Class::Fog,
            values: volume.values.iter().map(|v| (0.5 - v / (2.0 * volume.voxel)).clamp(0.0, 1.0)).collect(),
            background: 0.0,
            ..volume.clone()
        },
    }
}

// Iso-surface as triangles (naive surface nets): level sets at distance `iso`,
// fog volumes where density crosses `iso`.
pub fn to_polygons(volume: &Volume, iso: f32) -> Mesh {
    let [nx, ny, nz] = volume.dims;
    let field = |x: usize, y: usize, z: usize| {
        let v = volume.get(x, y, z);
        match volume.class {
            Class::LevelSet => v - iso,
            Class::Fog => iso - v,
        }
    };
    let mut mesh = Mesh::default();
    let mut cube_vertex: HashMap<(usize, usize, usize), usize> = HashMap::new();
    let edges = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let corner = |i: usize| (i & 1, (i >> 1) & 1, (i >> 2) & 1);
    for z in 0..nz.saturating_sub(1) {
        for y in 0..ny.saturating_sub(1) {
            for x in 0..nx.saturating_sub(1) {
                let values: [f32; 8] = std::array::from_fn(|i| {
                    let (dx, dy, dz) = corner(i);
                    field(x + dx, y + dy, z + dz)
                });
                let inside = values.iter().filter(|v| **v < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                // The cube's vertex is the average of its edge crossings.
                let mut sum = Vec3::zero();
                let mut count = 0.0;
                for (a, b) in edges {
                    let (va, vb) = (values[a], values[b]);
                    if (va < 0.0) != (vb < 0.0) {
                        let t = va / (va - vb);
                        let (ca, cb) = (corner(a), corner(b));
                        let lerp = |p: usize, q: usize| p as f32 + (q as f32 - p as f32) * t;
                        sum = sum.add(Vec3::new(lerp(ca.0, cb.0), lerp(ca.1, cb.1), lerp(ca.2, cb.2)));
                        count += 1.0;
                    }
                }
                let local = sum.scale(1.0 / count);
                let base = volume.center(x, y, z);
                cube_vertex.insert((x, y, z), mesh.vertices.len());
                mesh.vertices.push(base.add(local.scale(volume.voxel)));
            }
        }
    }
    // Each grid edge crossing the surface becomes a quad joining the four cubes around it.
    let mut quad = |cubes: [(usize, usize, usize); 4], flip: bool, mesh: &mut Mesh| {
        let ids: Option<Vec<usize>> = cubes.iter().map(|c| cube_vertex.get(c).copied()).collect();
        if let Some(ids) = ids {
            let [a, b, c, d] = [ids[0], ids[1], ids[2], ids[3]];
            if flip {
                mesh.indices.push([a, c, b]);
                mesh.indices.push([a, d, c]);
            } else {
                mesh.indices.push([a, b, c]);
                mesh.indices.push([a, c, d]);
            }
        }
    };
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let here = field(x, y, z) < 0.0;
                if x + 1 < nx && y > 0 && z > 0 && here != (field(x + 1, y, z) < 0.0) {
                    quad([(x, y - 1, z - 1), (x, y, z - 1), (x, y, z), (x, y - 1, z)], !here, &mut mesh);
                }
                if y + 1 < ny && x > 0 && z > 0 && here != (field(x, y + 1, z) < 0.0) {
                    quad([(x - 1, y, z - 1), (x - 1, y, z), (x, y, z), (x, y, z - 1)], !here, &mut mesh);
                }
                if z + 1 < nz && x > 0 && y > 0 && here != (field(x, y, z + 1) < 0.0) {
                    quad([(x - 1, y - 1, z), (x, y - 1, z), (x, y, z), (x - 1, y, z)], !here, &mut mesh);
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sum over triangles of a·(b×c): its sign tells whether faces wind outward like the app's primitives.
    fn signed_volume(mesh: &Mesh) -> f32 {
        mesh.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            a.dot(b.cross(c))
        }).sum::<f32>() / 6.0
    }

    #[test]
    fn sphere_to_sdf_and_back_keeps_shape_and_winding() {
        let sphere = super::super::node_sphere(1.0, 24, 48);
        let sdf = from_polygons(&sphere, 0.08, 3.0).unwrap();
        let surface = to_polygons(&sdf, 0.0);
        assert!(!surface.indices.is_empty());
        for v in &surface.vertices {
            assert!((v.length() - 1.0).abs() < 0.08, "vertex {v:?} should lie on the unit sphere");
        }
        let (reference, rebuilt) = (signed_volume(&sphere), signed_volume(&surface));
        assert!(reference.signum() == rebuilt.signum(), "winding: sphere {reference}, rebuilt {rebuilt}");
        assert!((rebuilt.abs() - 4.18879).abs() < 0.25, "volume {rebuilt}");
    }

    #[test]
    fn volumes_survive_the_point_encoding() {
        let sphere = super::super::node_sphere(1.0, 16, 32);
        let sdf = from_polygons(&sphere, 0.1, 3.0).unwrap();
        let points = to_mesh(&sdf);
        assert!(is_volume(&points));
        let back = from_mesh(&points).unwrap();
        // The flood fill recovers the inside even though only the band was stored.
        let centre_value = {
            let c = [0, 1, 2].map(|a| ((0.0 - [back.origin.x, back.origin.y, back.origin.z][a]) / back.voxel).round() as usize);
            back.get(c[0], c[1], c[2])
        };
        assert!(centre_value < 0.0);
        let rebuilt = to_polygons(&back, 0.0);
        assert!((signed_volume(&rebuilt).abs() - 4.18879).abs() < 0.3);
    }

    #[test]
    fn filled_distances_reach_deep_inside() {
        // A closed box of side 2: from_polygons only knows distances near its faces.
        let mut mesh = Mesh::default();
        for k in 0..8 {
            mesh.vertices.push(Vec3::new((k & 1) as f32 * 2.0 - 1.0, ((k >> 1) & 1) as f32 * 2.0 - 1.0, ((k >> 2) & 1) as f32 * 2.0 - 1.0));
        }
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            mesh.indices.push([f[0], f[1], f[2]]);
            mesh.indices.push([f[0], f[2], f[3]]);
        }
        let mut sdf = from_polygons(&mesh, 0.05, 3.0).unwrap();
        let centre = Vec3::new(0.0, 0.0, 0.0);
        assert!(sdf.gradient(centre).length() < 1e-3, "narrow band: no gradient deep inside");
        fill_distances(&mut sdf);
        assert!((sdf.sample(centre) + 1.0).abs() < 0.12, "centre is ~1 inside: {}", sdf.sample(centre));
        let p = Vec3::new(0.0, 0.6, 0.0);
        assert!((sdf.sample(p) + 0.4).abs() < 0.08, "{}", sdf.sample(p));
        assert!(sdf.gradient(p).y > 0.5, "the gradient points out through the nearest face");
        assert!(sdf.sample(Vec3::new(0.0, 1.1, 0.0)) > 0.0, "outside stays positive");
    }

    #[test]
    fn fog_iso_surface_and_offset() {
        let sphere = super::super::node_sphere(1.0, 16, 32);
        let mut sdf = from_polygons(&sphere, 0.1, 3.0).unwrap();
        let fog = to_fog(&sdf);
        assert_eq!(fog.class, Class::Fog);
        let from_fog = to_polygons(&fog, 0.5);
        let radius = from_fog.vertices.iter().map(|v| v.length()).sum::<f32>() / from_fog.vertices.len() as f32;
        assert!((radius - 1.0).abs() < 0.1, "fog iso radius {radius}");
        offset(&mut sdf, 0.2);
        let grown = to_polygons(&sdf, 0.0);
        let radius = grown.vertices.iter().map(|v| v.length()).sum::<f32>() / grown.vertices.len() as f32;
        assert!((radius - 1.2).abs() < 0.1, "offset radius {radius}");
    }
}
