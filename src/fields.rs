// Volume Wrangle and Heightfield Wrangle, and the field sampling behind VEX's
// volumesample() family.
//
// Volumes are point-per-voxel geometry (volume.rs): VEX runs over the voxels with
// @density (fog) or @surface (level set) and @P at the voxel centre. Terrains are
// grids with the height in P.y (terrain.rs): VEX runs over the grid points with
// @height and @mask. Positions can't move in either: a wrangle changes values only.

use super::{Mesh, Vec3};
use crate::{terrain, vex, volume};
use std::collections::HashMap;

// A sampled field: the voxels of a volume, or the heights of a terrain.
pub enum Field {
    Voxels { map: HashMap<[i64; 3], f32>, origin: Vec3, voxel: f32, background: f32 },
    // Row by row along x, then z; (x0, z0) is the first point.
    Height { nx: usize, nz: usize, x0: f32, z0: f32, dx: f32, dz: f32, values: Vec<f32> },
}

fn attr<'a>(mesh: &'a Mesh, name: &str) -> Option<&'a Vec<f32>> {
    mesh.point_attributes.get(name).filter(|v| v.len() == mesh.vertices.len())
}

impl Field {
    // The field `name` of `mesh`: a volume's density/surface (or any float point
    // attribute on its voxels), or a terrain's height/mask (or any point attribute).
    pub fn from_mesh(mesh: &Mesh, name: &str) -> Result<Field, String> {
        if let Some(info) = mesh.point_attributes.get(terrain::INFO).filter(|i| i.len() >= 4) {
            let (nx, nz) = (info[0] as usize, info[1] as usize);
            if nx < 2 || nz < 2 || nx * nz != mesh.vertices.len() {
                return Err("the terrain's points were added or removed, so it's no longer a grid".into());
            }
            let values = match (name, attr(mesh, name)) {
                (_, Some(v)) => v.clone(),
                ("height", None) => mesh.vertices.iter().map(|p| p.y).collect(),
                ("mask", None) => vec![0.0; nx * nz],
                _ => return Err(format!("the terrain has no \"{name}\" layer (it has height, mask and its point attributes)")),
            };
            return Ok(Field::Height { nx, nz, x0: mesh.vertices[0].x, z0: mesh.vertices[0].z, dx: info[2], dz: info[3], values });
        }
        let (voxels, class, voxel, origin, background) =
            volume::decode_voxels(mesh).map_err(|_| "volumesample() needs a volume (VDB, Pyro, Convert VDB) or a terrain".to_string())?;
        let class_name = if class == volume::Class::Fog { "density" } else { "surface" };
        let (values, background) = if name == class_name {
            (voxels.iter().map(|(_, v)| *v).collect::<Vec<_>>(), background)
        } else {
            (attr(mesh, name).cloned().ok_or_else(|| format!("the volume has no \"{name}\" field (it has {class_name})"))?, 0.0)
        };
        let map = voxels.iter().zip(values).map(|((index, _), v)| (*index, v)).collect();
        Ok(Field::Voxels { map, origin, voxel, background })
    }

    // Continuous index of a world position (voxel or grid units).
    pub fn pos_to_index(&self, p: Vec3) -> Vec3 {
        match self {
            Field::Voxels { origin, voxel, .. } => p.sub(*origin).scale(1.0 / voxel),
            Field::Height { x0, z0, dx, dz, .. } => Vec3::new((p.x - x0) / dx, (p.z - z0) / dz, 0.0),
        }
    }

    pub fn index_to_pos(&self, i: Vec3) -> Vec3 {
        match self {
            Field::Voxels { origin, voxel, .. } => origin.add(i.scale(*voxel)),
            Field::Height { x0, z0, dx, dz, .. } => Vec3::new(x0 + i.x * dx, 0.0, z0 + i.y * dz),
        }
    }

    // The value at a whole index; outside the active voxels, the background (a
    // terrain clamps to its edge).
    pub fn at_index(&self, i: [i64; 3]) -> f32 {
        match self {
            Field::Voxels { map, background, .. } => map.get(&i).copied().unwrap_or(*background),
            Field::Height { nx, nz, values, .. } => {
                let x = i[0].clamp(0, *nx as i64 - 1) as usize;
                let z = i[1].clamp(0, *nz as i64 - 1) as usize;
                values[x + nx * z]
            }
        }
    }

    // Trilinear (volumes) or bilinear (terrains) interpolation at a world position.
    pub fn sample(&self, p: Vec3) -> f32 {
        let i = self.pos_to_index(p);
        let (fx, fy, fz) = (i.x.floor(), i.y.floor(), i.z.floor());
        let (tx, ty, tz) = (i.x - fx, i.y - fy, i.z - fz);
        let (x, y, z) = (fx as i64, fy as i64, fz as i64);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let plane = |z: i64| {
            let a = lerp(self.at_index([x, y, z]), self.at_index([x + 1, y, z]), tx);
            let b = lerp(self.at_index([x, y + 1, z]), self.at_index([x + 1, y + 1, z]), tx);
            lerp(a, b, ty)
        };
        match self {
            Field::Height { .. } => plane(0),
            Field::Voxels { .. } => lerp(plane(z), plane(z + 1), tz),
        }
    }

    // Central differences, per world unit. A terrain's gradient lies in XZ.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        let d = |axis: Vec3, h: f32| (self.sample(p.add(axis.scale(h))) - self.sample(p.sub(axis.scale(h)))) / (2.0 * h);
        match self {
            Field::Voxels { voxel, .. } => Vec3::new(d(Vec3::new(1.0, 0.0, 0.0), *voxel), d(Vec3::new(0.0, 1.0, 0.0), *voxel), d(Vec3::new(0.0, 0.0, 1.0), *voxel)),
            Field::Height { dx, dz, .. } => Vec3::new(d(Vec3::new(1.0, 0.0, 0.0), *dx), 0.0, d(Vec3::new(0.0, 0.0, 1.0), *dz)),
        }
    }
}

// Whether `code` assigns to @name (or one of its components, @name.x / @name[0]).
fn writes(code: &str, name: &str) -> bool {
    let pattern = format!("@{name}");
    code.match_indices(&pattern).any(|(at, _)| {
        let mut rest = &code[at + pattern.len()..];
        if rest.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return false;
        }
        // Skip a component: .x, .rgb or [2].
        if let Some(after) = rest.strip_prefix('.') {
            rest = after.trim_start_matches(|c: char| c.is_alphanumeric());
        } else if rest.starts_with('[') {
            rest = rest.find(']').map_or("", |end| &rest[end + 1..]);
        }
        let rest = rest.trim_start();
        (rest.starts_with('=') && !rest.starts_with("==")) || ["+=", "-=", "*=", "/="].iter().any(|op| rest.starts_with(op))
    })
}

// VEX over a volume's voxels: @density or @surface, @P (the voxel centre, read-only),
// and any other float attributes the code creates (they become extra fields).
pub fn volume_wrangle(input: Mesh, inputs: &[Mesh], code: &str, time: f32, time_inc: f32, channels: Option<&vex::Channels>) -> Result<Mesh, String> {
    let (_, class, voxel, _, _) = volume::decode_voxels(&input).map_err(|_| "Volume Wrangle works on volumes: VDB, Pyro or Convert VDB output".to_string())?;
    let positions = input.vertices.clone();
    let mut out = vex::run_wrangle(input, inputs, code, time, time_inc, 2, None, channels);
    let name = if class == volume::Class::Fog { "density" } else { "surface" };
    // Voxels can be removed (removepoint deactivates them) but not moved or added.
    if out.vertices.len() > positions.len() {
        return Err("a Volume Wrangle can't add voxels (addpoint): it changes values only".into());
    }
    if out.vertices.len() == positions.len() {
        out.vertices = positions;
    }
    let values = attr(&out, name).cloned().ok_or_else(|| format!("the volume lost its {name} values"))?;
    // The viewport look follows the new values, unless the code set it itself.
    let keep_alpha = writes(code, "Alpha");
    let keep_cd = writes(code, "Cd");
    let mut alpha = Vec::with_capacity(values.len());
    let mut colours = Vec::with_capacity(values.len() * 3);
    for v in &values {
        let (a, c) = volume::display(class, voxel, *v);
        alpha.push(a);
        colours.extend(c);
    }
    if !keep_alpha {
        out.point_attributes.insert("Alpha".into(), alpha);
    }
    if !keep_cd {
        out.point_attributes.insert("Cd".into(), colours);
    }
    Ok(out)
}

// VEX over a terrain's points: @height (P.y), @mask, @P (x and z read-only) and the
// terrain's other point attributes. Writing @height or @P.y changes the height.
pub fn heightfield_wrangle(mut input: Mesh, inputs: &[Mesh], code: &str, time: f32, time_inc: f32, mask: Option<&[bool]>, channels: Option<&vex::Channels>) -> Result<Mesh, String> {
    Field::from_mesh(&input, "height").map_err(|e| if e.contains("no longer a grid") { e } else { "Heightfield Wrangle works on terrains: start from a Terrain node".to_string() })?;
    let n = input.vertices.len();
    let before: Vec<f32> = input.vertices.iter().map(|p| p.y).collect();
    let positions = input.vertices.clone();
    input.point_attributes.insert("height".into(), before.clone());
    if attr(&input, "mask").is_none() {
        input.point_attributes.insert("mask".into(), vec![0.0; n]);
    }
    let mut out = vex::run_wrangle(input, inputs, code, time, time_inc, 2, mask, channels);
    if out.vertices.len() != n {
        return Err("a Heightfield Wrangle can't add or remove points: it changes heights and layers only".into());
    }
    let height = out.point_attributes.remove("height").filter(|h| h.len() == n).unwrap_or_else(|| before.clone());
    for (i, p) in out.vertices.iter_mut().enumerate() {
        // @height wins where it changed; otherwise keep whatever @P.y became.
        let y = if (height[i] - before[i]).abs() > 0.0 { height[i] } else { p.y };
        *p = Vec3::new(positions[i].x, if y.is_finite() { y } else { before[i] }, positions[i].z);
    }
    out.point_attributes.remove("N");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fog_cube() -> Mesh {
        let mut voxels = Vec::new();
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    voxels.push(([x, y, z], 1.0));
                }
            }
        }
        volume::encode_voxels(&voxels, volume::Class::Fog, 0.5, Vec3::zero(), 0.0)
    }

    #[test]
    fn volume_wrangle_changes_values_not_positions() {
        let cube = fog_cube();
        let out = volume_wrangle(cube.clone(), &[], "@density *= @P.y; @P.x += 10;", 0.0, 1.0 / 24.0, None).unwrap();
        assert_eq!(out.vertices, cube.vertices);
        let d = &out.point_attributes["density"];
        for (p, v) in out.vertices.iter().zip(d) {
            assert!((v - p.y).abs() < 1e-5);
        }
        // The viewport look follows: empty voxels are clear.
        let alpha = &out.point_attributes["Alpha"];
        let bottom = out.vertices.iter().position(|p| p.y == 0.0).unwrap();
        assert_eq!(alpha[bottom], 0.0);
        // Removing voxels deactivates them.
        let fewer = volume_wrangle(cube.clone(), &[], "if (@P.y > 1) removepoint(0, @ptnum);", 0.0, 1.0 / 24.0, None).unwrap();
        assert_eq!(fewer.vertices.len(), 48);
        assert!(volume_wrangle(crate::node_box(1.0, 1), &[], "@density = 1;", 0.0, 1.0, None).is_err());
    }

    #[test]
    fn volumesample_reads_neighbours() {
        let cube = fog_cube();
        // Average of the voxel and its +x neighbour, read from the input.
        let out = volume_wrangle(cube.clone(), &[], "@density = volumesample(0, \"density\", @P + set(0.25, 0, 0)) * @P.x;", 0.0, 1.0, None).unwrap();
        let d = &out.point_attributes["density"];
        for (p, v) in out.vertices.iter().zip(d) {
            // Inside, the field is 1; at the +x edge, half of it is background.
            let expect = if p.x > 1.4 { 0.5 } else { 1.0 } * p.x;
            assert!((v - expect).abs() < 1e-4, "{p:?} {v}");
        }
        let field = Field::from_mesh(&cube, "density").unwrap();
        assert_eq!(field.at_index([9, 9, 9]), 0.0);
        let g = field.gradient(Vec3::new(1.5, 0.75, 0.75));
        assert!(g.x < -0.5 && g.y.abs() < 1e-5, "{g:?}");
        assert!(Field::from_mesh(&cube, "temperature").is_err());
    }

    #[test]
    fn heightfield_wrangle_sets_heights_and_layers() {
        let t = terrain::terrain(&terrain::TerrainSettings { size: [4.0, 4.0], resolution: 8 });
        let out = heightfield_wrangle(t.clone(), &[], "@height = @P.x; @mask = @P.z > 0;", 0.0, 1.0, None, None).unwrap();
        for (p, q) in out.vertices.iter().zip(&t.vertices) {
            assert!((p.y - q.x).abs() < 1e-5 && p.x == q.x && p.z == q.z);
        }
        assert!(!out.point_attributes.contains_key("height"));
        let mask = &out.point_attributes["mask"];
        assert!(mask.iter().zip(&out.vertices).all(|(m, p)| (*m > 0.5) == (p.z > 0.0)));
        // @P.y works too, and neighbours come from volumesample.
        let lifted = heightfield_wrangle(out.clone(), &[], "@P.y = volumesample(0, \"height\", @P + set(0.5, 0, 0));", 0.0, 1.0, None, None).unwrap();
        let p = lifted.vertices[3];
        assert!((p.y - (p.x + 0.5)).abs() < 1e-4, "{p:?}");
        assert!(heightfield_wrangle(crate::node_box(1.0, 1), &[], "@height = 1;", 0.0, 1.0, None, None).is_err());
        assert!(heightfield_wrangle(t, &[], "removepoint(0, 0);", 0.0, 1.0, None, None).is_err());
    }

    #[test]
    fn writes_detects_assignments() {
        assert!(writes("@Cd = {1,0,0};", "Cd"));
        assert!(writes("@Cd.x += 1;", "Cd"));
        assert!(writes("@Cd[1] = 0;", "Cd"));
        assert!(!writes("float r = @Cd.x;", "Cd"));
        assert!(!writes("if (@Alpha == 1) {}", "Alpha"));
        assert!(!writes("@Cdx = 1;", "Cd"));
    }
}
