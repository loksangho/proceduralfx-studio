// Extrude, like Houdini's PolyExtrude.
//
// Faces (all, or the node's group) are pushed out along their normals by
// `distance` in `divisions` steps, shrunk by `inset`, with side walls built
// between the original edges and the moved ones.
// - Connected: faces that share points stay together (one cap, walls only
//   around the outside of each connected region); points move along their
//   averaged normals.
// - Individual: every face extrudes on its own, with its own walls.
// Outputs can each be turned off: the front (moved faces), the back (the
// original faces, flipped), the sides. Each can go in a primitive group.
// Faces outside the group are kept as they are. Point and primitive
// attributes are carried over to the new points and faces.

use super::{Mesh, Vec3};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ExtrudeSettings {
    pub distance: f32,
    // Fraction (0..1) each moved face shrinks towards its centre.
    pub inset: f32,
    pub divisions: usize,
    pub connected: bool,
    pub output_front: bool,
    pub output_back: bool,
    pub output_side: bool,
    pub make_groups: bool,
    pub front_group: String,
    pub side_group: String,
    pub back_group: String,
}

impl ExtrudeSettings {
    pub fn defaults() -> Self {
        ExtrudeSettings {
            distance: 0.2,
            inset: 0.0,
            divisions: 1,
            connected: true,
            output_front: true,
            output_back: false,
            output_side: true,
            make_groups: true,
            front_group: "extrudeFront".into(),
            side_group: "extrudeSide".into(),
            back_group: "extrudeBack".into(),
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.distance,
            self.inset,
            // The old "mode": 0 individual, 1 connected.
            self.connected as u8 as f32,
            self.divisions as f32,
            self.output_front as u8 as f32,
            self.output_back as u8 as f32,
            self.output_side as u8 as f32,
            self.make_groups as u8 as f32,
        ]
    }

    // Old scenes saved [distance, inset, mode] and kept the original faces.
    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        let old = p.len() <= 3;
        ExtrudeSettings {
            distance: g(0, d.distance),
            inset: g(1, 0.0).clamp(0.0, 1.0),
            connected: g(2, 1.0) > 0.5,
            divisions: (g(3, 1.0).max(1.0) as usize).min(100),
            output_front: g(4, 1.0) > 0.5,
            output_back: if old { true } else { g(5, 0.0) > 0.5 },
            output_side: g(6, 1.0) > 0.5,
            make_groups: if old { false } else { g(7, 1.0) > 0.5 },
            ..d
        }
    }
}

fn face_normal(m: &Mesh, t: usize) -> Vec3 {
    let [a, b, c] = m.indices[t].map(|i| m.vertices[i]);
    b.sub(a).cross(c.sub(a))
}

pub fn extrude(input: &Mesh, selected: Option<&[bool]>, set: &ExtrudeSettings) -> Mesh {
    let (np, nt) = (input.vertices.len(), input.indices.len());
    let chosen: Vec<bool> = (0..nt).map(|t| selected.is_none_or(|s| s.get(t).copied().unwrap_or(false))).collect();
    let divisions = set.divisions.clamp(1, 100);
    let mut out = Mesh { vertices: input.vertices.clone(), ..Mesh::default() };
    // Where each new point and face comes from, for attributes.
    let mut point_source: Vec<usize> = (0..np).collect();
    // (corners, source face, 0 kept / 1 front / 2 side / 3 back, output polygon).
    // Kept, front and back faces are their source polygon's; each wall quad is its own.
    let polygon_of = crate::poly::of_triangles(input);
    let mut faces: Vec<([usize; 3], usize, u8, usize)> = Vec::new();
    let mut walls = 0usize;
    let face_polygon = |kind: u8, t: usize| kind as usize * nt + polygon_of[t];
    for t in 0..nt {
        if !chosen[t] {
            faces.push((input.indices[t], t, 0, face_polygon(0, t)));
        }
    }
    // Layers of moved points: layer[k][p] for k in 1..=divisions (layer 0 is the original point).
    let mut add_point = |out: &mut Mesh, point_source: &mut Vec<usize>, p: Vec3, from: usize| {
        out.vertices.push(p);
        point_source.push(from);
        out.vertices.len() - 1
    };
    // Individual: each polygon extrudes on its own.
    let regions: Vec<Vec<usize>> = if set.connected {
        vec![(0..nt).filter(|t| chosen[*t]).collect()]
    } else {
        crate::poly::runs(input).into_iter().map(|run| run.filter(|t| chosen[*t]).collect::<Vec<_>>()).filter(|r| !r.is_empty()).collect()
    };
    for region in regions {
        if region.is_empty() {
            continue;
        }
        // Each region point's normal (area weighted) and the centre it shrinks towards.
        let mut normal: HashMap<usize, Vec3> = HashMap::new();
        let mut centre: HashMap<usize, (Vec3, f32)> = HashMap::new();
        for &t in &region {
            let n = face_normal(input, t);
            let tri = input.indices[t];
            let c = tri.iter().fold(Vec3::zero(), |s, &v| s.add(input.vertices[v])).scale(1.0 / 3.0);
            for &v in &tri {
                *normal.entry(v).or_insert(Vec3::zero()) = normal.get(&v).copied().unwrap_or(Vec3::zero()).add(n);
                let e = centre.entry(v).or_insert((Vec3::zero(), 0.0));
                e.0 = e.0.add(c);
                e.1 += 1.0;
            }
        }
        let mut points: Vec<usize> = normal.keys().copied().collect();
        points.sort_unstable();
        let mut layers: Vec<HashMap<usize, usize>> = vec![points.iter().map(|&p| (p, p)).collect()];
        for k in 1..=divisions {
            let f = k as f32 / divisions as f32;
            let mut layer = HashMap::new();
            for &p in &points {
                let n = normal[&p].normalize();
                let (sum, count) = centre[&p];
                let towards = sum.scale(1.0 / count).sub(input.vertices[p]);
                let moved = input.vertices[p].add(n.scale(set.distance * f)).add(towards.scale(set.inset.clamp(0.0, 1.0) * f));
                layer.insert(p, add_point(&mut out, &mut point_source, moved, p));
            }
            layers.push(layer);
        }
        let top = &layers[divisions];
        if set.output_front {
            for &t in &region {
                faces.push((input.indices[t].map(|v| top[&v]), t, 1, face_polygon(1, t)));
            }
        }
        if set.output_back {
            for &t in &region {
                let tri = input.indices[t];
                faces.push(([tri[2], tri[1], tri[0]], t, 3, face_polygon(3, t)));
            }
        }
        if set.output_side {
            // Walls along the region's outside edges (edges only one of its faces uses).
            let mut edge_use: HashMap<(usize, usize), usize> = HashMap::new();
            for &t in &region {
                let tri = input.indices[t];
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    *edge_use.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            for &t in &region {
                let tri = input.indices[t];
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    if edge_use[&(a.min(b), a.max(b))] != 1 {
                        continue;
                    }
                    for k in 0..divisions {
                        let (a0, b0, a1, b1) = (layers[k][&a], layers[k][&b], layers[k + 1][&a], layers[k + 1][&b]);
                        let wall = 4 * nt + walls;
                        walls += 1;
                        faces.push(([a0, b0, b1], t, 2, wall));
                        faces.push(([a0, b1, a1], t, 2, wall));
                    }
                }
            }
        }
    }
    // Drop points nothing uses any more (the originals of fully replaced faces stay, as Houdini keeps them).
    out.indices = faces.iter().map(|f| f.0).collect();
    let n_out = out.vertices.len();
    for (key, values) in &input.point_attributes {
        if key.starts_with("detail:") {
            out.point_attributes.insert(key.clone(), values.clone());
        } else if let Some(name) = key.strip_prefix("prim:") {
            if nt > 0 && values.len() % nt == 0 && !name.starts_with("group:") || (name.starts_with("group:") && values.len() == nt) {
                let w = values.len() / nt.max(1);
                out.point_attributes.insert(key.clone(), faces.iter().flat_map(|f| values[f.1 * w..f.1 * w + w].iter().copied()).collect());
            }
        } else if key.starts_with("vertex:") {
            // Corner attributes: kept and back faces reuse their source's corners; new faces copy the source's first.
            if nt > 0 && values.len() % (nt * 3) == 0 {
                let w = values.len() / (nt * 3);
                out.point_attributes.insert(
                    key.clone(),
                    faces
                        .iter()
                        .flat_map(|f| {
                            let base = f.1 * 3 * w;
                            let order: [usize; 3] = if f.2 == 3 { [2, 1, 0] } else { [0, 1, 2] };
                            order.iter().flat_map(move |k| values[base + k * w..base + k * w + w].to_vec()).collect::<Vec<_>>()
                        })
                        .collect(),
                );
            }
        } else if np > 0 && values.len() % np == 0 {
            let w = values.len() / np;
            out.point_attributes.insert(key.clone(), (0..n_out).flat_map(|p| values[point_source[p] * w..point_source[p] * w + w].iter().copied()).collect());
        }
    }
    if set.make_groups {
        for (kind, name, on) in [(1u8, &set.front_group, set.output_front), (2, &set.side_group, set.output_side), (3, &set.back_group, set.output_back)] {
            if on && !name.trim().is_empty() {
                out.point_attributes.insert(format!("prim:group:{}", name.trim()), faces.iter().map(|f| (f.2 == kind) as u8 as f32).collect());
            }
        }
    }
    out.point_attributes.remove("N");
    // Output polygons: runs of faces from the same polygon.
    let mut ids = Vec::with_capacity(faces.len());
    let mut next = 0.0f32;
    for (i, f) in faces.iter().enumerate() {
        if i == 0 || faces[i - 1].3 != f.3 {
            next += 1.0;
        }
        ids.push(next);
    }
    out.point_attributes.insert(crate::poly::KEY.into(), ids);
    crate::poly::normalize(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1x1 square (two triangles) facing +y.
    fn square() -> Mesh {
        let mut m = crate::node_grid(1.0, 1.0, 2, 2);
        m.point_attributes.insert("Cd".into(), (0..12).map(|i| i as f32).collect());
        m.point_attributes.insert("prim:tag".into(), vec![7.0, 8.0]);
        m
    }

    fn volume(m: &Mesh) -> f32 {
        m.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| m.vertices[i]);
            a.dot(b.cross(c)) / 6.0
        }).sum()
    }

    #[test]
    fn connected_extrusion_makes_a_closed_box_with_groups() {
        let set = ExtrudeSettings { output_back: true, distance: 0.5, ..ExtrudeSettings::defaults() };
        let out = extrude(&square(), None, &set);
        // Front 2 + back 2 + 4 walls x 2 = 12 triangles, no wall between the two triangles.
        assert_eq!(out.indices.len(), 12);
        assert!((volume(&out).abs() - 0.5).abs() < 1e-4, "a closed 1 x 1 x 0.5 box: {}", volume(&out));
        let front = &out.point_attributes["prim:group:extrudeFront"];
        let side = &out.point_attributes["prim:group:extrudeSide"];
        let back = &out.point_attributes["prim:group:extrudeBack"];
        assert_eq!((front.iter().sum::<f32>(), side.iter().sum::<f32>(), back.iter().sum::<f32>()), (2.0, 8.0, 2.0));
        // Attributes come along: front faces keep their tag, new points their colour.
        let tags: Vec<f32> = out.point_attributes["prim:tag"].iter().zip(front).filter(|(_, f)| **f > 0.5).map(|(t, _)| *t).collect();
        assert_eq!(tags, vec![7.0, 8.0]);
        assert_eq!(out.point_attributes["Cd"].len(), out.vertices.len() * 3);
        assert_eq!(out.point_attributes["Cd"][4 * 3..4 * 3 + 3], out.point_attributes["Cd"][..3]);
    }

    #[test]
    fn outputs_divisions_individual_and_groups() {
        // No back (the default): the original faces are replaced.
        let open = extrude(&square(), None, &ExtrudeSettings::defaults());
        assert_eq!(open.indices.len(), 2 + 8);
        let no_sides = extrude(&square(), None, &ExtrudeSettings { output_side: false, ..ExtrudeSettings::defaults() });
        assert_eq!(no_sides.indices.len(), 2);
        let divided = extrude(&square(), None, &ExtrudeSettings { divisions: 3, ..ExtrudeSettings::defaults() });
        assert_eq!(divided.indices.len(), 2 + 8 * 3);
        // The square is one quad: a front quad and four wall quads.
        assert_eq!(crate::poly::count(&open), 1 + 4);
        let individual = extrude(&square(), None, &ExtrudeSettings { connected: false, ..ExtrudeSettings::defaults() });
        assert_eq!((individual.indices.len(), crate::poly::count(&individual)), (2 + 4 * 2, 5));
        // As two separate triangles, each gets all three walls.
        let mut triangles = square();
        crate::poly::clear(&mut triangles);
        let individual = extrude(&triangles, None, &ExtrudeSettings { connected: false, ..ExtrudeSettings::defaults() });
        assert_eq!((individual.indices.len(), crate::poly::count(&individual)), (2 + 2 * 3 * 2, 2 + 6));
        // Only the group: the other face is untouched.
        let one = extrude(&square(), Some(&[true, false]), &ExtrudeSettings::defaults());
        assert_eq!(one.indices.len(), 1 + 1 + 6);
        // Old saves keep their look: originals kept, no groups.
        let old = ExtrudeSettings::from_params(&[0.2, 0.1, 0.0]);
        assert!(old.output_back && !old.make_groups && !old.connected);
        let s = ExtrudeSettings::defaults();
        assert_eq!(ExtrudeSettings::from_params(&s.to_params()), s);
    }
}
