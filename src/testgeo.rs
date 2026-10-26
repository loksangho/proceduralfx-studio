// Test Geometry: ready-made shapes to start from, like Houdini's test geometry.
//
// Spot (a cow), Bob (a rubber toy) and Blub (a fish) are Keenan Crane's public-domain
// (CC0) models, built into the app with their colour textures baked into Cd; the
// animated character is Quaternius's CC0 "Animated Human" (see assets/testgeo/README.md).
// The Platonic solids, the torus knot and the shader ball are generated.

use super::{Mesh, Vec3};
use std::sync::OnceLock;

pub const SHAPES: [&str; 10] = ["Spot (cow)", "Bob (rubber toy)", "Blub (fish)", "Tetrahedron", "Octahedron", "Dodecahedron", "Icosahedron", "Torus knot", "Shader ball", "Character (animated)"];

// The shape number of the animated character.
pub const CHARACTER: usize = 9;

// The character's colour palettes (skin and outfit).
pub const LOOKS: [&str; 6] = ["Light skin, outfit 1", "Dark skin, outfit 1", "Light skin, outfit 2", "Dark skin, outfit 2", "Light skin, outfit 3", "Dark skin, outfit 3"];
const LOOK_TEXTURES: [&[u8]; 6] = [
    include_bytes!("../assets/testgeo/character_ClothedLightSkin.png"),
    include_bytes!("../assets/testgeo/character_ClothedDarkSkin.png"),
    include_bytes!("../assets/testgeo/character_ClothedLightSkin1.png"),
    include_bytes!("../assets/testgeo/character_ClothedDarkSkin1.png"),
    include_bytes!("../assets/testgeo/character_ClothedLightSkin2.png"),
    include_bytes!("../assets/testgeo/character_ClothedDarkSkin2.png"),
];
const CHARACTER_FBX: &[u8] = include_bytes!("../assets/testgeo/character.fbx");

#[derive(Clone, Debug, PartialEq)]
pub struct TestGeoSettings {
    pub shape: usize,
    // The largest side, in scene units.
    pub size: f32,
    // Sit on the ground (y = 0) rather than centre on the origin.
    pub on_ground: bool,
    // The models' painted colours (Cd from their textures).
    pub colours: bool,
    // Torus knot: windings around the axis (p) and through the hole (q), and detail.
    pub p: usize,
    pub q: usize,
    pub detail: usize,
    // Character: which take plays (see character_clips()), its look, and whether the
    // take repeats or holds its last pose.
    pub clip: usize,
    pub look: usize,
    pub looping: bool,
}

impl TestGeoSettings {
    pub fn defaults() -> Self {
        TestGeoSettings { shape: 0, size: 2.0, on_ground: true, colours: true, p: 2, q: 3, detail: 48, clip: 1, look: 0, looping: true }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.shape as f32,
            self.size,
            self.on_ground as u8 as f32,
            self.colours as u8 as f32,
            self.p as f32,
            self.q as f32,
            self.detail as f32,
            self.clip as f32,
            self.look as f32,
            self.looping as u8 as f32,
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        TestGeoSettings {
            shape: (g(0, 0.0).max(0.0) as usize).min(SHAPES.len() - 1),
            size: g(1, d.size).max(1e-3),
            on_ground: g(2, 1.0) != 0.0,
            colours: g(3, 1.0) != 0.0,
            p: g(4, d.p as f32).clamp(1.0, 12.0) as usize,
            q: g(5, d.q as f32).clamp(1.0, 12.0) as usize,
            detail: g(6, d.detail as f32).clamp(8.0, 512.0) as usize,
            clip: g(7, d.clip as f32).clamp(0.0, 64.0) as usize,
            look: (g(8, 0.0).max(0.0) as usize).min(LOOKS.len() - 1),
            looping: g(9, 1.0) != 0.0,
        }
    }
}

// A built-in model: its OBJ and colour texture.
struct Model {
    obj: &'static str,
    texture: &'static [u8],
}

const MODELS: [Model; 3] = [
    Model { obj: include_str!("../assets/testgeo/spot.obj"), texture: include_bytes!("../assets/testgeo/spot.png") },
    Model { obj: include_str!("../assets/testgeo/bob.obj"), texture: include_bytes!("../assets/testgeo/bob.png") },
    Model { obj: include_str!("../assets/testgeo/blub.obj"), texture: include_bytes!("../assets/testgeo/blub.png") },
];

// Each model read once, with its texture sampled into vertex:Cd.
fn model(index: usize) -> Result<&'static Mesh, String> {
    static CACHE: [OnceLock<Result<Mesh, String>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    CACHE[index]
        .get_or_init(|| {
            let m = &MODELS[index];
            let mut mesh = crate::fileio::read_obj(m.obj)?;
            bake_colours(&mut mesh, m.texture)?;
            Ok(mesh)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

// A convex solid around the origin; every face is wound to face outward (the way the
// app's own shapes are).
// Samples a texture at each corner's uv into vertex:Cd.
fn bake_colours(mesh: &mut Mesh, texture: &[u8]) -> Result<(), String> {
    let image = image::load_from_memory(texture).map_err(|e| e.to_string())?.to_rgb8();
    let Some(uv) = mesh.point_attributes.get("vertex:uv").filter(|uv| uv.len() == mesh.indices.len() * 9) else {
        return Ok(());
    };
    let (w, h) = (image.width() as f32, image.height() as f32);
    let mut cd = Vec::with_capacity(uv.len());
    for corner in uv.chunks_exact(3) {
        // UVs are the unit square; v goes up the image.
        let x = (corner[0].rem_euclid(1.0) * w).min(w - 1.0) as u32;
        let y = ((1.0 - corner[1].rem_euclid(1.0)) * h).min(h - 1.0) as u32;
        let px = image.get_pixel(x, y).0;
        // As painted: the viewport shows Cd as is, like the app's other colours.
        cd.extend(px.map(|c| c as f32 / 255.0));
    }
    mesh.point_attributes.insert("vertex:Cd".into(), cd);
    Ok(())
}

// The character's named takes (the exporter's unnamed "ArmatureAction" leftovers
// aren't): (stack number, name), in file order.
fn character_takes(scene: &ufbx::Scene) -> Vec<(usize, String)> {
    crate::fileio::fbx_takes(scene).into_iter().filter(|(_, t)| !t.name.starts_with("ArmatureAction")).map(|(i, t)| (i, t.name)).collect()
}

// The animated character's takes (Idle, Walk, Run…), in file order.
pub fn character_clips() -> Vec<String> {
    crate::fileio::fbx_scene_from_memory("builtin:character", CHARACTER_FBX).map(|scene| character_takes(&scene).into_iter().map(|(_, n)| n).collect()).unwrap_or_default()
}

// The character at `time` seconds into its take, and its rest pose (which sets the
// scale and placement, so a jump really leaves the ground).
fn character(set: &TestGeoSettings, time: f32) -> Result<(Mesh, Mesh), String> {
    let scene = crate::fileio::fbx_scene_from_memory("builtin:character", CHARACTER_FBX)?;
    let takes = character_takes(&scene);
    let take = takes.get(set.clip.min(takes.len().saturating_sub(1))).map(|(i, _)| *i);
    let mut posed = crate::fileio::fbx_evaluate(&scene, take, time, set.looping)?;
    let rest = crate::fileio::fbx_evaluate(&scene, take, 0.0, false)?;
    if set.colours {
        bake_colours(&mut posed, LOOK_TEXTURES[set.look.min(LOOKS.len() - 1)])?;
    }
    Ok((posed, rest))
}

fn polyhedron(points: &[[f32; 3]], faces: &[&[usize]]) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices = points.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect();
    for face in faces {
        let v = |i: usize| mesh.vertices[face[i]];
        let normal = v(1).sub(v(0)).cross(v(2).sub(v(0)));
        let centre = face.iter().fold(Vec3::zero(), |a, &i| a.add(mesh.vertices[i]));
        let mut face = face.to_vec();
        if (normal.dot(centre) < 0.0) != INWARD {
            face.reverse();
        }
        crate::poly::add(&mut mesh, &face);
    }
    mesh
}

// The app's winding: whether (b - a) × (c - a) of a triangle points into the shape.
const INWARD: bool = false;

fn platonic(which: usize) -> Mesh {
    let phi = (1.0 + 5f32.sqrt()) / 2.0;
    match which {
        0 => polyhedron(&[[1.0, 1.0, 1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, -1.0], [1.0, -1.0, -1.0]], &[&[0, 1, 3], &[0, 2, 1], &[0, 3, 2], &[1, 2, 3]]),
        1 => polyhedron(
            &[[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]],
            &[&[0, 2, 4], &[2, 1, 4], &[1, 3, 4], &[3, 0, 4], &[2, 0, 5], &[1, 2, 5], &[3, 1, 5], &[0, 3, 5]],
        ),
        2 => {
            // The dodecahedron as the dual of the icosahedron: a point per icosahedron face.
            let ico = platonic(3);
            let faces: Vec<Vec<usize>> = crate::poly::runs(&ico).iter().map(|r| crate::poly::points(&ico, r)).collect();
            let centres: Vec<[f32; 3]> = faces.iter().map(|f| {
                let c = f.iter().fold(Vec3::zero(), |a, &i| a.add(ico.vertices[i])).scale(1.0 / f.len() as f32);
                [c.x, c.y, c.z]
            }).collect();
            // Around each icosahedron vertex, the faces that touch it, in order.
            let mut pentagons: Vec<Vec<usize>> = Vec::new();
            for v in 0..ico.vertices.len() {
                let mut ring: Vec<usize> = (0..faces.len()).filter(|&f| faces[f].contains(&v)).collect();
                let axis = ico.vertices[v].normalize();
                let c0 = Vec3::from_arr(centres[ring[0]]);
                let e1 = c0.sub(axis.scale(c0.dot(axis))).normalize();
                let e2 = axis.cross(e1);
                ring.sort_by(|&a, &b| {
                    let angle = |f: usize| {
                        let c = Vec3::from_arr(centres[f]);
                        c.dot(e2).atan2(c.dot(e1))
                    };
                    angle(a).total_cmp(&angle(b))
                });
                pentagons.push(ring);
            }
            let faces: Vec<&[usize]> = pentagons.iter().map(|p| p.as_slice()).collect();
            polyhedron(&centres, &faces)
        }
        _ => polyhedron(
            &[
                [-1.0, phi, 0.0], [1.0, phi, 0.0], [-1.0, -phi, 0.0], [1.0, -phi, 0.0],
                [0.0, -1.0, phi], [0.0, 1.0, phi], [0.0, -1.0, -phi], [0.0, 1.0, -phi],
                [phi, 0.0, -1.0], [phi, 0.0, 1.0], [-phi, 0.0, -1.0], [-phi, 0.0, 1.0],
            ],
            &[
                &[0, 11, 5], &[0, 5, 1], &[0, 1, 7], &[0, 7, 10], &[0, 10, 11],
                &[1, 5, 9], &[5, 11, 4], &[11, 10, 2], &[10, 7, 6], &[7, 1, 8],
                &[3, 9, 4], &[3, 4, 2], &[3, 2, 6], &[3, 6, 8], &[3, 8, 9],
                &[4, 9, 5], &[2, 4, 11], &[6, 2, 10], &[8, 6, 7], &[9, 8, 1],
            ],
        ),
    }
}

// A (p, q) torus knot: a tube following a curve that winds p times around the axis
// and q times through the hole.
fn torus_knot(p: usize, q: usize, detail: usize) -> Mesh {
    let (p, q) = (p as f32, q as f32);
    let along = detail * 4;
    let around = (detail / 4).max(6);
    let curve = |t: f32| {
        let r = 2.0 + (q * t).cos();
        Vec3::new(r * (p * t).cos(), -(q * t).sin(), r * (p * t).sin())
    };
    let tau = std::f32::consts::TAU;
    let mut mesh = Mesh::default();
    for i in 0..along {
        let t = i as f32 / along as f32 * tau;
        let c = curve(t);
        let tangent = curve(t + 1e-3).sub(curve(t - 1e-3)).normalize();
        // A frame from the curve's bend (it never straightens on a torus knot).
        let bend = curve(t + 1e-3).add(curve(t - 1e-3)).sub(c.scale(2.0)).normalize();
        let side = tangent.cross(bend).normalize();
        for j in 0..around {
            let a = j as f32 / around as f32 * tau;
            mesh.vertices.push(c.add(bend.scale(0.4 * a.cos())).add(side.scale(0.4 * a.sin())));
        }
    }
    for i in 0..along {
        for j in 0..around {
            let at = |i: usize, j: usize| (i % along) * around + j % around;
            crate::poly::add(&mut mesh, &[at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)]);
        }
    }
    mesh
}

// A shader ball: a sphere on a short pedestal, with a band cut around it, for looking
// at materials.
fn shader_ball(detail: usize) -> Mesh {
    let rows = (detail / 2).max(8);
    let cols = detail.max(12);
    let mut ball = crate::node_sphere(1.0, rows, cols);
    for v in ball.vertices.iter_mut() {
        // A groove around the middle.
        let band = (-(v.y * 9.0).powi(2)).exp();
        let k = 1.0 - 0.06 * band;
        *v = Vec3::new(v.x * k, v.y + 1.35, v.z * k);
    }
    let mut stand = crate::node_tube(0.55, 0.35, 0.4, 2, cols, true);
    for v in stand.vertices.iter_mut() {
        v.y += 0.2;
    }
    crate::node_merge(stand, ball)
}

// The shape at `time` (seconds; only the character moves), scaled so its largest side
// is `size`, centred in x and z, and on the ground or centred in y.
pub fn generate(set: &TestGeoSettings, time: f32) -> Result<Mesh, String> {
    let (mut mesh, reference) = match set.shape {
        0..=2 => {
            let mut m = model(set.shape)?.clone();
            if !set.colours {
                m.point_attributes.remove("vertex:Cd");
            }
            (m, None)
        }
        3..=6 => (platonic(set.shape - 3), None),
        7 => (torus_knot(set.p, set.q, set.detail), None),
        8 => (shader_ball(set.detail), None),
        _ => {
            let (posed, rest) = character(set, time)?;
            (posed, Some(rest))
        }
    };
    let (lo, hi) = reference.as_ref().unwrap_or(&mesh).bounds().ok_or("the shape is empty")?;
    let extent = hi.sub(lo);
    let scale = set.size / extent.x.max(extent.y).max(extent.z).max(1e-6);
    let centre = lo.add(hi).scale(0.5);
    let floor = if set.on_ground { lo.y } else { centre.y };
    for v in mesh.vertices.iter_mut() {
        *v = Vec3::new((v.x - centre.x) * scale, (v.y - floor) * scale, (v.z - centre.z) * scale);
    }
    mesh.point_attributes.remove("N");
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_builds_at_its_size() {
        for shape in 0..SHAPES.len() {
            let set = TestGeoSettings { shape, ..TestGeoSettings::defaults() };
            let mesh = generate(&set, 0.0).unwrap_or_else(|e| panic!("{}: {e}", SHAPES[shape]));
            let (lo, hi) = mesh.bounds().unwrap();
            let e = hi.sub(lo);
            assert!((e.x.max(e.y).max(e.z) - 2.0).abs() < 1e-3, "{}: {e:?}", SHAPES[shape]);
            assert!(lo.y.abs() < 1e-4, "{} sits on the ground", SHAPES[shape]);
            assert!(crate::poly::count(&mesh) > 3, "{}", SHAPES[shape]);
        }
    }

    #[test]
    fn the_character_plays_its_takes() {
        let clips = character_clips();
        assert_eq!(clips, ["Death", "Idle", "Jump", "Punch", "Run", "Walk", "Working"]);
        let walk = |t: f32| generate(&TestGeoSettings { shape: CHARACTER, clip: 5, ..TestGeoSettings::defaults() }, t).unwrap();
        let (a, b) = (walk(0.0), walk(0.5));
        assert_eq!(a.vertices.len(), b.vertices.len());
        assert_ne!(a.vertices, b.vertices, "the walk moves");
        // Looping: one take's length later, it's the same pose again.
        let scene = crate::fileio::fbx_scene_from_memory("builtin:character", CHARACTER_FBX).unwrap();
        let walk_stack = character_takes(&scene)[5].0;
        let take = crate::fileio::fbx_takes(&scene).into_iter().find(|(i, _)| *i == walk_stack).unwrap().1;
        assert_eq!(take.name, "Walk");
        let length = (take.end - take.begin) as f32;
        let (mid, again) = (walk(0.3), walk(0.3 + length));
        let far = mid.vertices.iter().zip(&again.vertices).map(|(p, q)| p.sub(*q).length()).fold(0.0f32, f32::max);
        assert!(far < 1e-3, "{far}");
        // Coloured from its palette; standing on the ground at about the chosen size.
        assert!(a.point_attributes.contains_key("vertex:Cd"));
        let (lo, hi) = a.bounds().unwrap();
        assert!(lo.y.abs() < 0.05 && (hi.sub(lo).y - 2.0).abs() < 0.3, "{lo:?} {hi:?}");
        // A jump leaves the ground (the rest pose sets the placement, not each frame).
        let jump = |t: f32| generate(&TestGeoSettings { shape: CHARACTER, clip: 2, ..TestGeoSettings::defaults() }, t).unwrap();
        let lowest = (0..24).map(|f| jump(f as f32 / 24.0).bounds().unwrap().0.y).fold(f32::MIN, f32::max);
        assert!(lowest > 0.05, "{lowest}");
    }

    #[test]
    fn models_have_their_colours_and_solids_their_faces() {
        let spot = generate(&TestGeoSettings::defaults(), 0.0).unwrap();
        assert_eq!(spot.vertices.len(), 2930);
        assert_eq!(crate::poly::count(&spot), 2928, "quads stay quads");
        let cd = &spot.point_attributes["vertex:Cd"];
        assert_eq!(cd.len(), spot.indices.len() * 9);
        // Spot is mostly white with dark spots: both appear.
        assert!(cd.chunks(3).any(|c| c[0] > 0.8) && cd.chunks(3).any(|c| c[0] < 0.1));
        let plain = generate(&TestGeoSettings { colours: false, ..TestGeoSettings::defaults() }, 0.0).unwrap();
        assert!(!plain.point_attributes.contains_key("vertex:Cd"));
        let faces = |shape| crate::poly::count(&generate(&TestGeoSettings { shape, ..TestGeoSettings::defaults() }, 0.0).unwrap());
        assert_eq!([faces(3), faces(4), faces(5), faces(6)], [4, 8, 12, 20]);
        let dodeca = generate(&TestGeoSettings { shape: 5, ..TestGeoSettings::defaults() }, 0.0).unwrap();
        assert!(crate::poly::runs(&dodeca).iter().all(|r| crate::poly::points(&dodeca, r).len() == 5));
        assert_eq!(dodeca.vertices.len(), 20);
        // Faces point the way the app's own box's do.
        let outward = |m: &Mesh| {
            let (lo, hi) = m.bounds().unwrap();
            let c = lo.add(hi).scale(0.5);
            m.indices.iter().all(|t| {
                let [a, b, d] = t.map(|i| m.vertices[i]);
                let n = b.sub(a).cross(d.sub(a));
                (n.dot(a.add(b).add(d).scale(1.0 / 3.0).sub(c)) > 0.0) != INWARD
            })
        };
        assert!(outward(&crate::node_box(1.0, 1)), "INWARD matches the app's box");
        for shape in 3..=6 {
            assert!(outward(&generate(&TestGeoSettings { shape, ..TestGeoSettings::defaults() }, 0.0).unwrap()), "{}", SHAPES[shape]);
        }
    }
}
