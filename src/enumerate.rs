// Enumerate node: numbers points or primitives 0, 1, 2… (from Start, by Increment)
// in an attribute. With a piece attribute, elements sharing its value share one
// number, so pieces are numbered in the order they first appear.

use super::{poly, Mesh};

pub const CLASSES: [&str; 2] = ["Points", "Primitives"];

#[derive(Clone, Debug, PartialEq)]
pub struct EnumerateSettings {
    pub class: usize,
    // The attribute written.
    pub attribute: String,
    // Optional: number pieces (elements with the same value of this) instead of elements.
    pub piece_attribute: String,
    pub start: f32,
    pub increment: f32,
}

impl EnumerateSettings {
    pub fn defaults() -> EnumerateSettings {
        EnumerateSettings { class: 0, attribute: "index".into(), piece_attribute: String::new(), start: 0.0, increment: 1.0 }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.class as f32, self.start, self.increment]
    }

    pub fn from_params(p: &[f32]) -> EnumerateSettings {
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        EnumerateSettings {
            class: (g(0, 0.0).max(0.0) as usize).min(1),
            start: g(1, 0.0),
            increment: g(2, 1.0),
            ..EnumerateSettings::defaults()
        }
    }
}

pub fn enumerate(input: &Mesh, set: &EnumerateSettings) -> Result<Mesh, String> {
    let name = set.attribute.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err("enter one attribute name to write the numbers to".into());
    }
    let points = set.class == 0;
    if !points && input.indices.is_empty() {
        return Err("there are no primitives to number; set Class to Points".into());
    }
    let polys = poly::runs(input);
    let count = if points { input.vertices.len() } else { polys.len() };
    // Each element's piece: its own index, or the piece attribute's (first) value.
    let by = set.piece_attribute.trim();
    let keys: Vec<u32> = if by.is_empty() {
        (0..count as u32).collect()
    } else {
        let read = |key: &str, n: usize| input.point_attributes.get(key).filter(|v| n > 0 && !v.is_empty() && v.len() % n == 0).map(|v| (v, v.len() / n));
        let values: Vec<f32> = if points {
            read(by, input.vertices.len()).map(|(v, w)| (0..count).map(|i| v[i * w]).collect())
        } else if let Some((v, w)) = read(&format!("prim:{by}"), input.indices.len()) {
            Some(polys.iter().map(|run| v[run.start * w]).collect())
        } else {
            // A point attribute still works for primitives: their first point's value.
            read(by, input.vertices.len()).map(|(v, w)| polys.iter().map(|run| v[input.indices[run.start][0] * w]).collect())
        }
        .ok_or_else(|| format!("there's no attribute called {by} to number the pieces by"))?;
        let mut seen = std::collections::HashMap::new();
        values.iter().map(|v| {
            let next = seen.len() as u32;
            *seen.entry(v.to_bits()).or_insert(next)
        }).collect()
    };
    let numbers: Vec<f32> = keys.iter().map(|&k| set.start + k as f32 * set.increment).collect();
    let mut out = input.clone();
    if points {
        out.point_attributes.insert(name.to_owned(), numbers);
    } else {
        let mut prim = vec![0.0; input.indices.len()];
        for (run, n) in polys.iter().zip(&numbers) {
            prim[run.clone()].fill(*n);
        }
        out.point_attributes.insert(format!("prim:{name}"), prim);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    fn two_quads() -> Mesh {
        let mut mesh = Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0)],
            indices: vec![[0, 1, 2], [0, 2, 3], [3, 2, 5], [3, 5, 4]],
            ..Mesh::default()
        };
        poly::tag_quads(&mut mesh, 0, 2);
        mesh
    }

    #[test]
    fn numbers_points_and_polygons() {
        let mesh = two_quads();
        let points = enumerate(&mesh, &EnumerateSettings { start: 10.0, increment: 2.0, ..EnumerateSettings::defaults() }).unwrap();
        assert_eq!(points.point_attributes["index"], vec![10.0, 12.0, 14.0, 16.0, 18.0, 20.0]);
        let prims = enumerate(&mesh, &EnumerateSettings { class: 1, attribute: "variant".into(), ..EnumerateSettings::defaults() }).unwrap();
        // A quad is one primitive: both its triangles get its number.
        assert_eq!(prims.point_attributes["prim:variant"], vec![0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn a_piece_attribute_numbers_the_pieces() {
        let mut mesh = two_quads();
        mesh.point_attributes.insert("prim:name".into(), vec![7.5, 7.5, 3.0, 3.0]);
        let set = EnumerateSettings { class: 1, attribute: "piece".into(), piece_attribute: "name".into(), ..EnumerateSettings::defaults() };
        assert_eq!(enumerate(&mesh, &set).unwrap().point_attributes["prim:piece"], vec![0.0, 0.0, 1.0, 1.0]);
        mesh.point_attributes.insert("cluster".into(), vec![5.0, 5.0, 9.0, 5.0, 9.0, 9.0]);
        let set = EnumerateSettings { piece_attribute: "cluster".into(), ..EnumerateSettings::defaults() };
        assert_eq!(enumerate(&mesh, &set).unwrap().point_attributes["index"], vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0]);
        assert!(enumerate(&mesh, &EnumerateSettings { piece_attribute: "nope".into(), ..EnumerateSettings::defaults() }).is_err());
    }
}
