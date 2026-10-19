// Measure: geometric quantities as attributes.
//
// - Area / Perimeter: per primitive (`prim:<name>`).
// - Point Area: each point's share of its triangles (a third of each).
// - Curvature: per point, mean (cotangent Laplacian) or Gaussian (angle deficit).
// - Gradient: per point, the gradient of a float point attribute (a vector).
// - Volume: of the closed mesh, as a detail attribute.
// - Curve Length: per point, the length of the curve it's on (hair, polylines).
// A summary (sum, average, min or max) can also go to `detail:<name>`.

use super::{Mesh, Vec3};

pub const KINDS: [&str; 7] = ["Area", "Perimeter", "Point Area", "Curvature", "Gradient", "Volume", "Curve Length"];
pub const SUMMARIES: [&str; 5] = ["None", "Sum", "Average", "Minimum", "Maximum"];
pub const CURVATURES: [&str; 2] = ["Mean", "Gaussian"];

#[derive(Clone, Debug, PartialEq)]
pub struct MeasureSettings {
    pub kind: usize,
    // Output attribute name (empty: the kind's own, e.g. area).
    pub name: String,
    // Gradient: the float point attribute to differentiate.
    pub source: String,
    pub curvature: usize,
    pub summary: usize,
}

impl MeasureSettings {
    pub fn new(kind: usize) -> Self {
        MeasureSettings { kind, name: String::new(), source: "height".into(), curvature: 0, summary: if matches!(kind, 0 | 1) { 1 } else { 0 } }
    }

    pub fn output_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_owned();
        }
        ["area", "perimeter", "area", "curvature", "gradient", "volume", "length"][self.kind.min(6)].to_owned()
    }
}

fn tri(mesh: &Mesh, t: usize) -> [Vec3; 3] {
    mesh.indices[t].map(|i| mesh.vertices[i])
}

fn area(p: [Vec3; 3]) -> f32 {
    p[1].sub(p[0]).cross(p[2].sub(p[0])).length() * 0.5
}

fn angle_at(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let (u, v) = (b.sub(a).normalize(), c.sub(a).normalize());
    u.dot(v).clamp(-1.0, 1.0).acos()
}

fn cot(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    // Cotangent of the angle at a in triangle abc.
    let (u, v) = (b.sub(a), c.sub(a));
    u.dot(v) / u.cross(v).length().max(1e-12)
}

fn point_areas(mesh: &Mesh) -> Vec<f32> {
    let mut out = vec![0.0; mesh.vertices.len()];
    for t in 0..mesh.indices.len() {
        let a = area(tri(mesh, t)) / 3.0;
        for &v in &mesh.indices[t] {
            out[v] += a;
        }
    }
    out
}

pub fn measure(input: &Mesh, set: &MeasureSettings) -> Result<Mesh, String> {
    let mut out = input.clone();
    let name = set.output_name();
    let (np, nt) = (input.vertices.len(), input.indices.len());
    let needs_prims = !matches!(set.kind, 6);
    if needs_prims && nt == 0 {
        return Err("the input has no polygons to measure".into());
    }
    // (values, components, key) written; the summary reads the first component (or vector length).
    let (values, width, key): (Vec<f32>, usize, String) = match set.kind {
        0 => ((0..nt).map(|t| area(tri(input, t))).collect(), 1, format!("prim:{name}")),
        1 => (
            (0..nt)
                .map(|t| {
                    let p = tri(input, t);
                    p[1].sub(p[0]).length() + p[2].sub(p[1]).length() + p[0].sub(p[2]).length()
                })
                .collect(),
            1,
            format!("prim:{name}"),
        ),
        2 => (point_areas(input), 1, name.clone()),
        3 => {
            let areas = point_areas(input);
            let values = if set.curvature == 1 {
                // Gaussian: angle deficit over the point's area (boundary points get 0).
                let mut angles = vec![0.0f32; np];
                let mut edges: std::collections::HashMap<(usize, usize), i32> = Default::default();
                for t in 0..nt {
                    let [a, b, c] = input.indices[t];
                    let [pa, pb, pc] = tri(input, t);
                    angles[a] += angle_at(pa, pb, pc);
                    angles[b] += angle_at(pb, pc, pa);
                    angles[c] += angle_at(pc, pa, pb);
                    for (x, y) in [(a, b), (b, c), (c, a)] {
                        *edges.entry((x.min(y), x.max(y))).or_default() += 1;
                    }
                }
                let mut boundary = vec![false; np];
                for ((x, y), n) in edges {
                    if n == 1 {
                        boundary[x] = true;
                        boundary[y] = true;
                    }
                }
                (0..np).map(|i| if boundary[i] || areas[i] <= 0.0 { 0.0 } else { (std::f32::consts::TAU - angles[i]) / areas[i] }).collect()
            } else {
                // Mean: half the cotangent Laplacian of P, signed by the normal (positive on spheres).
                let mut lap = vec![Vec3::zero(); np];
                for t in 0..nt {
                    let [a, b, c] = input.indices[t];
                    let [pa, pb, pc] = tri(input, t);
                    for (i, j, k, pi, pj, pk) in [(a, b, c, pa, pb, pc), (b, c, a, pb, pc, pa), (c, a, b, pc, pa, pb)] {
                        // The edge j-k gets the cotangent of the angle at i.
                        let w = cot(pi, pj, pk) * 0.5;
                        lap[j] = lap[j].add(pk.sub(pj).scale(w));
                        lap[k] = lap[k].add(pj.sub(pk).scale(w));
                    }
                }
                let normals = input.compute_normals();
                (0..np)
                    .map(|i| {
                        if areas[i] <= 0.0 {
                            return 0.0;
                        }
                        let h = lap[i].scale(1.0 / (2.0 * areas[i]));
                        -h.dot(normals[i]).signum() * h.length()
                    })
                    .collect()
            };
            (values, 1, name.clone())
        }
        4 => {
            let source = set.source.trim();
            let f = input.point_attributes.get(source).filter(|v| v.len() == np).ok_or_else(|| format!("the input has no float point attribute `{source}`"))?;
            let mut grad = vec![Vec3::zero(); np];
            let mut weight = vec![0.0f32; np];
            for t in 0..nt {
                let [a, b, c] = input.indices[t];
                let [pa, pb, pc] = tri(input, t);
                let n = pb.sub(pa).cross(pc.sub(pa));
                let area2 = n.length();
                if area2 < 1e-12 {
                    continue;
                }
                let nn = n.scale(1.0 / area2);
                // Linear gradient over the triangle: sum of f_i * (n x e_i) / 2A, e_i opposite vertex i.
                let g = nn.cross(pc.sub(pb)).scale(f[a]).add(nn.cross(pa.sub(pc)).scale(f[b])).add(nn.cross(pb.sub(pa)).scale(f[c])).scale(1.0 / area2);
                for v in [a, b, c] {
                    grad[v] = grad[v].add(g.scale(area2));
                    weight[v] += area2;
                }
            }
            (grad.iter().zip(&weight).flat_map(|(g, w)| { let g = g.scale(1.0 / w.max(1e-12)); [g.x, g.y, g.z] }).collect(), 3, name.clone())
        }
        5 => {
            let v: f32 = (0..nt).map(|t| {
                let [a, b, c] = tri(input, t);
                a.dot(b.cross(c)) / 6.0
            }).sum();
            out.point_attributes.insert(format!("detail:{name}"), vec![v.abs()]);
            return Ok(out);
        }
        _ => {
            let curves = crate::groom::curves(input);
            if curves.is_empty() {
                return Err("the input has no curves (points with curveu, e.g. from Hair Generate)".into());
            }
            let mut values = vec![0.0f32; np];
            for range in curves {
                let len: f32 = input.vertices[range.clone()].windows(2).map(|w| w[1].sub(w[0]).length()).sum();
                values[range].iter_mut().for_each(|v| *v = len);
            }
            (values, 1, name.clone())
        }
    };
    let scalars: Vec<f32> = values.chunks(width).map(|c| if width == 1 { c[0] } else { c.iter().map(|x| x * x).sum::<f32>().sqrt() }).collect();
    if set.summary > 0 && !scalars.is_empty() {
        // Perimeter's sum is the mesh's open boundary length, not the sum of triangle perimeters.
        let total = if set.kind == 1 && set.summary == 1 {
            let mut edges: std::collections::HashMap<(usize, usize), i32> = Default::default();
            for t in &input.indices {
                for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    *edges.entry((x.min(y), x.max(y))).or_default() += 1;
                }
            }
            edges.iter().filter(|(_, n)| **n == 1).map(|((x, y), _)| input.vertices[*x].sub(input.vertices[*y]).length()).sum()
        } else {
            match set.summary {
                1 => scalars.iter().sum(),
                2 => scalars.iter().sum::<f32>() / scalars.len() as f32,
                3 => scalars.iter().cloned().fold(f32::INFINITY, f32::min),
                _ => scalars.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            }
        };
        out.point_attributes.insert(format!("detail:{name}"), vec![total]);
    }
    out.point_attributes.insert(key, values);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(r: f32, rows: usize, cols: usize) -> Mesh {
        let mut m = Mesh::default();
        m.vertices.push(Vec3::new(0.0, r, 0.0));
        for i in 1..rows {
            let th = std::f32::consts::PI * i as f32 / rows as f32;
            for j in 0..cols {
                let ph = std::f32::consts::TAU * j as f32 / cols as f32;
                m.vertices.push(Vec3::new(r * th.sin() * ph.cos(), r * th.cos(), r * th.sin() * ph.sin()));
            }
        }
        m.vertices.push(Vec3::new(0.0, -r, 0.0));
        let bottom = m.vertices.len() - 1;
        let ring = |i: usize, j: usize| 1 + (i - 1) * cols + j % cols;
        for j in 0..cols {
            m.indices.push([0, ring(1, j + 1), ring(1, j)]);
            m.indices.push([bottom, ring(rows - 1, j), ring(rows - 1, j + 1)]);
        }
        for i in 1..rows - 1 {
            for j in 0..cols {
                let (a, b, c, d) = (ring(i, j), ring(i, j + 1), ring(i + 1, j), ring(i + 1, j + 1));
                m.indices.push([a, b, d]);
                m.indices.push([a, d, c]);
            }
        }
        m
    }

    #[test]
    fn areas_volumes_and_curvature_match_a_sphere() {
        let s = sphere(2.0, 40, 80);
        let area = measure(&s, &MeasureSettings::new(0)).unwrap();
        let total = area.point_attributes["detail:area"][0];
        assert!((total - 4.0 * std::f32::consts::PI * 4.0).abs() / total < 0.01, "area {total}");
        assert_eq!(area.point_attributes["prim:area"].len(), s.indices.len());
        let vol = measure(&s, &MeasureSettings::new(5)).unwrap().point_attributes["detail:volume"][0];
        assert!((vol - 4.0 / 3.0 * std::f32::consts::PI * 8.0).abs() / vol < 0.01, "volume {vol}");
        let mean = measure(&s, &MeasureSettings { summary: 2, ..MeasureSettings::new(3) }).unwrap().point_attributes["detail:curvature"][0];
        assert!((mean - 0.5).abs() < 0.03, "mean curvature 1/r: {mean}");
        let gauss = measure(&s, &MeasureSettings { curvature: 1, summary: 2, ..MeasureSettings::new(3) }).unwrap().point_attributes["detail:curvature"][0];
        assert!((gauss - 0.25).abs() < 0.02, "gaussian 1/r²: {gauss}");
        let pa: f32 = measure(&s, &MeasureSettings::new(2)).unwrap().point_attributes["area"].iter().sum();
        assert!((pa - total).abs() < 1e-2);
    }

    #[test]
    fn gradients_perimeters_and_curve_lengths() {
        // A flat 2 x 2 grid with height = 3x: gradient (3, 0, 0) everywhere.
        let mut grid = crate::node_grid(2.0, 2.0, 5, 5);
        let h: Vec<f32> = grid.vertices.iter().map(|p| 3.0 * p.x).collect();
        grid.point_attributes.insert("height".into(), h);
        let g = measure(&grid, &MeasureSettings::new(4)).unwrap();
        for c in g.point_attributes["gradient"].chunks(3) {
            assert!((c[0] - 3.0).abs() < 1e-3 && c[1].abs() < 1e-3 && c[2].abs() < 1e-3, "{c:?}");
        }
        let per = measure(&grid, &MeasureSettings::new(1)).unwrap().point_attributes["detail:perimeter"][0];
        assert!((per - 8.0).abs() < 1e-3, "boundary of a 2x2 square: {per}");
        assert!(measure(&grid, &MeasureSettings { source: "nope".into(), ..MeasureSettings::new(4) }).is_err());
        let mut curve = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 3.0, 0.0)], ..Mesh::default() };
        curve.point_attributes.insert(crate::groom::CURVEU.into(), vec![0.0, 0.5, 1.0]);
        let l = measure(&curve, &MeasureSettings::new(6)).unwrap();
        assert_eq!(l.point_attributes["length"], vec![3.0, 3.0, 3.0]);
        assert!(measure(&Mesh::default(), &MeasureSettings::new(0)).is_err());
    }
}
