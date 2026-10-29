// Attributes by name, class and type, shared by Attribute Create, Randomize and Delete.
//
// Any name works, on points, primitives (polygons), vertices or the detail. The type says
// how many values each element holds and how the panels show them: a float of 1 to 4
// components, an integer, a vector (3), a colour (3) or a quaternion (4, x y z w).
// Quaternions are entered as rotations in degrees, turning around X, then Y, then Z,
// the same order as Transform.

use super::{poly, Mesh};

// In this order so older scenes keep their meaning: Detail was third before Vertices came.
pub const CLASSES: [&str; 4] = ["Points", "Primitives", "Detail", "Vertices"];
pub const TYPES: [&str; 5] = ["Float", "Integer", "Vector", "Colour", "Quaternion"];
pub const INTEGER: usize = 1;
pub const VECTOR: usize = 2;
pub const COLOUR: usize = 3;
pub const QUATERNION: usize = 4;

// Attributes other nodes understand, as (name, type, components, a typical value).
// Quaternion values are rotations in degrees.
pub const COMMON: [(&str, usize, usize, [f32; 4]); 9] = [
    ("Cd", COLOUR, 3, [1.0, 1.0, 1.0, 0.0]),
    ("Alpha", 0, 1, [1.0, 0.0, 0.0, 0.0]),
    ("pscale", 0, 1, [0.1, 0.0, 0.0, 0.0]),
    ("N", VECTOR, 3, [0.0, 1.0, 0.0, 0.0]),
    ("v", VECTOR, 3, [0.0, 0.0, 0.0, 0.0]),
    ("up", VECTOR, 3, [0.0, 1.0, 0.0, 0.0]),
    ("orient", QUATERNION, 4, [0.0, 0.0, 0.0, 0.0]),
    ("id", INTEGER, 1, [0.0, 0.0, 0.0, 0.0]),
    ("emission", COLOUR, 3, [0.0, 0.0, 0.0, 0.0]),
];

// Values each element holds for a type (a float's own size, otherwise fixed).
pub fn components(kind: usize, size: usize) -> usize {
    match kind {
        VECTOR | COLOUR => 3,
        QUATERNION => 4,
        INTEGER => 1,
        _ => size.clamp(1, 4),
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

// The attribute's storage key on the geometry.
pub fn key(name: &str, class: usize) -> String {
    let name = name.trim();
    match class {
        1 => format!("prim:{name}"),
        2 => format!("detail:{name}"),
        3 => format!("vertex:{name}"),
        _ => name.to_owned(),
    }
}

// The elements of a class that each get their own value, and how many stored entries
// the attribute has: a polygon's value goes on all its triangles, vertices are triangle corners.
pub fn elements(mesh: &Mesh, class: usize) -> (Vec<std::ops::Range<usize>>, usize) {
    match class {
        1 => (poly::runs(mesh), mesh.indices.len()),
        2 => (vec![0..1], 1),
        3 => ((0..mesh.indices.len() * 3).map(|c| c..c + 1).collect(), mesh.indices.len() * 3),
        _ => ((0..mesh.vertices.len()).map(|p| p..p + 1).collect(), mesh.vertices.len()),
    }
}

// The rotation turning around X, then Y, then Z by these degrees, as a quaternion [x, y, z, w].
pub fn quaternion(degrees: [f32; 3]) -> [f32; 4] {
    let half = degrees.map(|d| d.to_radians() * 0.5);
    let axis = |i: usize| {
        let mut q = [0.0, 0.0, 0.0, half[i].cos()];
        q[i] = half[i].sin();
        q
    };
    // X happens first, so it's on the right.
    multiply(axis(2), multiply(axis(1), axis(0)))
}

// The rotation `b` followed by `a`.
pub fn multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateSettings {
    pub name: String,
    pub class: usize,
    pub kind: usize,
    pub size: usize,
    // Up to four components; a quaternion's first three are its rotation in degrees.
    pub value: [f32; 4],
}

// The first two numbers were the old attribute (0 id, 1 pscale, 2 Cd, 3 orient) and its
// value; 4 means the name, class and type that follow.
const BY_NAME: f32 = 4.0;

impl CreateSettings {
    pub fn defaults() -> Self {
        CreateSettings { name: "Cd".into(), class: 0, kind: COLOUR, size: 3, value: [1.0, 1.0, 1.0, 0.0] }
    }

    pub fn to_params(&self) -> Vec<f32> {
        let v = self.value;
        vec![BY_NAME, v[0], self.class as f32, self.kind as f32, self.size as f32, v[1], v[2], v[3]]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        if p.len() <= 2 {
            // An old save: id, pscale, a colour chosen by hue, or orient as one number, which
            // Copy to Points read as a turn about Y in degrees: now that turn as a quaternion.
            let value = g(1, 1.0);
            return match g(0, 1.0).max(0.0) as usize {
                0 => CreateSettings { name: "id".into(), class: 0, kind: INTEGER, size: 1, value: [value, 0.0, 0.0, 0.0] },
                2 => {
                    let [r, gr, b] = crate::attribute_color(value);
                    CreateSettings { name: "Cd".into(), class: 0, kind: COLOUR, size: 3, value: [r, gr, b, 0.0] }
                }
                3 => CreateSettings { name: "orient".into(), class: 0, kind: QUATERNION, size: 4, value: [0.0, value, 0.0, 0.0] },
                _ => CreateSettings { name: "pscale".into(), class: 0, kind: 0, size: 1, value: [value, 0.0, 0.0, 0.0] },
            };
        }
        CreateSettings {
            name: Self::defaults().name,
            class: (g(2, 0.0).max(0.0) as usize).min(CLASSES.len() - 1),
            kind: (g(3, 0.0).max(0.0) as usize).min(TYPES.len() - 1),
            size: (g(4, 1.0).max(1.0) as usize).min(4),
            value: [g(1, 0.0), g(5, 0.0), g(6, 0.0), g(7, 0.0)],
        }
    }

    // The values each element gets.
    pub fn stored(&self) -> Vec<f32> {
        let v = self.value;
        match self.kind {
            QUATERNION => quaternion([v[0], v[1], v[2]]).to_vec(),
            INTEGER => vec![v[0].round()],
            k => v[..components(k, self.size)].to_vec(),
        }
    }
}

// Sets the attribute on every element of its class (replacing it if it's there).
pub fn create(mut mesh: Mesh, set: &CreateSettings) -> Result<Mesh, String> {
    let name = set.name.trim();
    if !valid_name(name) {
        return Err(format!("'{name}' isn't an attribute name: use letters, digits and _"));
    }
    let (_, stored) = elements(&mesh, set.class);
    let value = set.stored();
    mesh.point_attributes.insert(key(name, set.class), value.iter().copied().cycle().take(stored * value.len()).collect());
    Ok(mesh)
}

// Attribute Delete: the attributes whose names match a pattern, on one class or all of them.
// The pattern is names separated by spaces, * matching any run of letters, and ^ in front
// of a name keeping it: "Cd N", "fx_*", "* ^P".
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteSettings {
    pub pattern: String,
    // A class (CLASSES), or ALL_CLASSES.
    pub class: usize,
}

pub const ALL_CLASSES: usize = 4;
const OLD_NAMES: [&str; 4] = ["id", "pscale", "Cd", "orient"];

impl DeleteSettings {
    pub fn defaults() -> Self {
        DeleteSettings { pattern: "pscale".into(), class: 0 }
    }

    // The old attribute number first (0 id, 1 pscale, 2 Cd, 3 orient), then the class;
    // 4 means the pattern, which travels as text.
    pub fn to_params(&self) -> Vec<f32> {
        vec![BY_NAME, self.class as f32]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let first = p.first().copied().filter(|v| v.is_finite()).unwrap_or(BY_NAME).max(0.0) as usize;
        if p.len() <= 1 {
            // An old save: one of four point attributes.
            return DeleteSettings { pattern: OLD_NAMES[first.min(3)].into(), class: 0 };
        }
        DeleteSettings { pattern: Self::defaults().pattern, class: (p[1].max(0.0) as usize).min(ALL_CLASSES) }
    }
}

// `name` matched against one word of a pattern, * matching any run of characters.
fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            let Some(rest) = name.strip_prefix(head) else { return false };
            (0..=rest.len()).filter(|i| rest.is_char_boundary(*i)).any(|i| glob(tail, &rest[i..]))
        }
    }
}

// Whether `name` matches the pattern: any word does and no ^ word does.
pub fn matches(pattern: &str, name: &str) -> bool {
    let words: Vec<&str> = pattern.split_whitespace().collect();
    words.iter().any(|w| !w.starts_with('^') && glob(w, name)) && !words.iter().any(|w| w.strip_prefix('^').is_some_and(|w| glob(w, name)))
}

// Removes the matching attributes; P (the points themselves) can't be removed.
pub fn delete(mut mesh: Mesh, set: &DeleteSettings) -> Mesh {
    let class_of = |key: &str| -> (usize, String) {
        for (class, prefix) in [(1, "prim:"), (2, "detail:"), (3, "vertex:")] {
            if let Some(name) = key.strip_prefix(prefix) {
                return (class, name.to_owned());
            }
        }
        (0, key.to_owned())
    };
    mesh.point_attributes.retain(|key, _| {
        let (class, name) = class_of(key);
        // Names that start with __ are the app's own bookkeeping (polygon outlines and such).
        let ours = name.starts_with("__");
        ours || !((set.class == ALL_CLASSES || set.class == class) && matches(&set.pattern, &name))
    });
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Mesh {
        crate::node_grid(2.0, 2.0, 3, 3)
    }

    #[test]
    fn any_attribute_of_any_type_on_any_class() {
        // A colour on points: three values each.
        let cd = create(grid(), &CreateSettings { value: [1.0, 0.5, 0.25, 0.0], ..CreateSettings::defaults() }).unwrap();
        assert_eq!(cd.point_attributes["Cd"], [1.0, 0.5, 0.25].repeat(9));
        // A custom vector on primitives, an integer on the detail, a float on vertices.
        let wind = create(grid(), &CreateSettings { name: "wind".into(), class: 1, kind: VECTOR, size: 3, value: [0.0, 2.0, -1.0, 0.0] }).unwrap();
        assert_eq!(wind.point_attributes["prim:wind"].len(), 8 * 3);
        let level = create(grid(), &CreateSettings { name: "level".into(), class: 2, kind: INTEGER, size: 1, value: [2.6, 0.0, 0.0, 0.0] }).unwrap();
        assert_eq!(level.point_attributes["detail:level"], [3.0]);
        let corner = create(grid(), &CreateSettings { name: "w".into(), class: 3, kind: 0, size: 2, value: [1.0, 2.0, 0.0, 0.0] }).unwrap();
        assert_eq!(corner.point_attributes["vertex:w"].len(), 8 * 3 * 2);
        assert!(create(grid(), &CreateSettings { name: "2bad".into(), ..CreateSettings::defaults() }).is_err());
    }

    #[test]
    fn orient_turns_copies() {
        // 90° about Z lays a tall box along X, as Copy to Points reads orient.
        let mut points = Mesh::default();
        points.vertices = vec![super::super::Vec3::new(5.0, 0.0, 0.0)];
        let set = CreateSettings { name: "orient".into(), class: 0, kind: QUATERNION, size: 4, value: [0.0, 0.0, 90.0, 0.0] };
        let points = create(points, &set).unwrap();
        let mut tall = crate::node_box(1.0, 1);
        tall.vertices.iter_mut().for_each(|v| v.y *= 3.0);
        let (lo, hi) = crate::node_copy_to_points(tall, points, 1.0, [0.0; 3]).bounds().unwrap();
        assert!((hi.x - lo.x - 3.0).abs() < 1e-4 && (hi.y - lo.y - 1.0).abs() < 1e-4, "{lo:?} {hi:?}");
    }

    #[test]
    fn orient_is_a_unit_quaternion_from_degrees() {
        let set = CreateSettings { name: "orient".into(), class: 0, kind: QUATERNION, size: 4, value: [0.0, 90.0, 0.0, 0.0] };
        let q = &create(grid(), &set).unwrap().point_attributes["orient"][..4];
        // 90° around Y: (0, sin 45°, 0, cos 45°).
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!(q.iter().zip([0.0, s, 0.0, s]).all(|(a, b)| (a - b).abs() < 1e-6), "{q:?}");
        // X turns first, then Y: X 90° then Y 90° takes +Z to +X... checked as composed rotations.
        let both = quaternion([90.0, 90.0, 0.0]);
        let expected = multiply(quaternion([0.0, 90.0, 0.0]), quaternion([90.0, 0.0, 0.0]));
        assert!(both.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6));
        assert!((both.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn delete_by_pattern_and_class() {
        let mut m = grid();
        for (key, size) in [("Cd", 27), ("fx_a", 9), ("fx_b", 9), ("N", 27), ("prim:Cd", 24), ("detail:fx_level", 1)] {
            m.point_attributes.insert(key.into(), vec![0.0; size]);
        }
        let keys = |m: &Mesh| { let mut k: Vec<String> = m.point_attributes.keys().filter(|k| !k.contains("__")).cloned().collect(); k.sort(); k };
        // Several names; only points by default.
        let out = delete(m.clone(), &DeleteSettings { pattern: "Cd N".into(), class: 0 });
        assert!(!keys(&out).contains(&"Cd".into()) && !keys(&out).contains(&"N".into()) && keys(&out).contains(&"prim:Cd".into()));
        // Wildcards, on every class.
        let fx = delete(m.clone(), &DeleteSettings { pattern: "fx_*".into(), class: ALL_CLASSES });
        assert!(keys(&fx).iter().all(|k| !k.contains("fx_")));
        // Everything but one, and the app's own bookkeeping stays.
        let all_but = delete(m.clone(), &DeleteSettings { pattern: "* ^N".into(), class: 0 });
        assert!(keys(&all_but).contains(&"N".into()) && !keys(&all_but).contains(&"Cd".into()));
        assert_eq!(m.point_attributes.keys().filter(|k| k.contains("__")).count(), all_but.point_attributes.keys().filter(|k| k.contains("__")).count());
        // Old saves deleted one of four point attributes.
        assert_eq!(DeleteSettings::from_params(&[2.0]), DeleteSettings { pattern: "Cd".into(), class: 0 });
        assert!(matches("p*e", "pscale") && !matches("p*e", "pscales") && matches("*", "anything"));
    }

    #[test]
    fn old_saves_load_as_the_attribute_they_made() {
        // Old orient stored one number, a turn about Y in degrees; it loads as that turn.
        let orient = CreateSettings::from_params(&[3.0, 90.0]);
        assert_eq!((orient.name.as_str(), orient.kind), ("orient", QUATERNION));
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!(orient.stored().iter().zip([0.0, s, 0.0, s]).all(|(a, b)| (a - b).abs() < 1e-6));
        let pscale = CreateSettings::from_params(&[1.0, 0.3]);
        assert_eq!((pscale.name.as_str(), pscale.stored()), ("pscale", vec![0.3]));
        let cd = CreateSettings::from_params(&[2.0, 0.0]);
        assert_eq!(cd.stored(), vec![1.0, 0.0, 0.0], "hue 0 was red");
        // New settings round-trip (the name travels as text).
        let s = CreateSettings { name: "Cd".into(), class: 3, kind: VECTOR, size: 3, value: [1.0, 2.0, 3.0, 4.0] };
        assert_eq!(CreateSettings::from_params(&s.to_params()), s);
    }
}
