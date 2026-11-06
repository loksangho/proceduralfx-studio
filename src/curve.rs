// Curve: a curve through (or near) points you place, as a polyline, a smooth curve that
// passes through every point (Catmull-Rom), or a smoother one drawn toward them (a cubic
// B-spline that starts and ends at the end points). Open or closed.
//
// Like every curve in the app, it's a run of points with `curveu` from 0 at its start to 1
// at its end (by length along it), so Resample, Sweep-style tools, POP Curve Force and the
// renderers all read it.

use super::{Mesh, Vec3};

pub const TYPES: [&str; 3] = ["Polyline", "Smooth (through the points)", "B-Spline (toward the points)"];

#[derive(Clone, Debug, PartialEq)]
pub struct CurveSettings {
    pub kind: usize,
    pub closed: bool,
    // Points per span between two control points (smooth types).
    pub divisions: usize,
    pub points: Vec<[f32; 3]>,
}

impl CurveSettings {
    pub fn defaults() -> CurveSettings {
        CurveSettings { kind: 1, closed: false, divisions: 12, points: vec![[-2.0, 0.0, 0.0], [-1.0, 1.0, 0.0], [1.0, -0.5, 0.0], [2.0, 0.5, 0.0]] }
    }

    pub fn params(&self) -> Vec<f32> {
        let mut p = vec![self.kind as f32, self.closed as u8 as f32, self.divisions as f32];
        p.extend(self.points.iter().flatten());
        p
    }

    pub fn from_params(p: &[f32]) -> CurveSettings {
        let g = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        CurveSettings {
            kind: (g(0, 1.0).max(0.0) as usize).min(TYPES.len() - 1),
            closed: g(1, 0.0) > 0.5,
            divisions: (g(2, 12.0).max(1.0) as usize).min(256),
            points: p.get(3..).unwrap_or(&[]).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
        }
    }
}

fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let (t2, t3) = (t * t, t * t * t);
    p1.scale(2.0)
        .add(p2.sub(p0).scale(t))
        .add(p0.scale(2.0).sub(p1.scale(5.0)).add(p2.scale(4.0)).sub(p3).scale(t2))
        .add(p1.scale(3.0).sub(p0).sub(p2.scale(3.0)).add(p3).scale(t3))
        .scale(0.5)
}

fn bspline(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let (t2, t3) = (t * t, t * t * t);
    let b0 = (1.0 - t).powi(3) / 6.0;
    let b1 = (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0;
    let b2 = (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0;
    let b3 = t3 / 6.0;
    p0.scale(b0).add(p1.scale(b1)).add(p2.scale(b2)).add(p3.scale(b3))
}

pub fn curve(set: &CurveSettings) -> Result<Mesh, String> {
    let cvs: Vec<Vec3> = set.points.iter().map(|p| Vec3::from_arr(*p)).collect();
    if cvs.len() < 2 {
        return Err("A curve needs at least two points".into());
    }
    let n = cvs.len();
    let closed = set.closed && n >= 3;
    let div = set.divisions.max(1);
    let mut out: Vec<Vec3> = Vec::new();
    match set.kind {
        1 => {
            // Through every point; past the ends, the end span is mirrored so it stays straight-ish.
            let at = |i: isize| -> Vec3 {
                if closed {
                    cvs[i.rem_euclid(n as isize) as usize]
                } else if i < 0 {
                    cvs[0].scale(2.0).sub(cvs[1])
                } else if i as usize >= n {
                    cvs[n - 1].scale(2.0).sub(cvs[n - 2])
                } else {
                    cvs[i as usize]
                }
            };
            let spans = if closed { n } else { n - 1 };
            for s in 0..spans as isize {
                for k in 0..div {
                    out.push(catmull_rom(at(s - 1), at(s), at(s + 1), at(s + 2), k as f32 / div as f32));
                }
            }
            out.push(if closed { cvs[0] } else { cvs[n - 1] });
        }
        2 => {
            // Open curves repeat their end points so they start and end on them.
            let controls: Vec<Vec3> = if closed {
                cvs.clone()
            } else {
                let mut c = vec![cvs[0], cvs[0]];
                c.extend(cvs.iter().copied());
                c.extend([cvs[n - 1], cvs[n - 1]]);
                c
            };
            let m = controls.len();
            let at = |i: usize| controls[if closed { i % m } else { i.min(m - 1) }];
            let spans = if closed { m } else { m - 3 };
            for s in 0..spans {
                for k in 0..div {
                    out.push(bspline(at(s), at(s + 1), at(s + 2), at(s + 3), k as f32 / div as f32));
                }
            }
            out.push(if closed { out[0] } else { cvs[n - 1] });
        }
        _ => {
            out = cvs.clone();
            if closed {
                out.push(cvs[0]);
            }
        }
    }
    // curveu by length along the curve.
    let mut along = vec![0.0f32];
    for w in out.windows(2) {
        along.push(along.last().unwrap() + w[1].sub(w[0]).length());
    }
    let total = along.last().copied().unwrap_or(0.0);
    let curveu: Vec<f32> = along.iter().enumerate().map(|(i, a)| if total > 1e-9 { a / total } else { i as f32 / (out.len() - 1).max(1) as f32 }).collect();
    let mut mesh = Mesh { vertices: out, ..Mesh::default() };
    mesh.point_attributes.insert(crate::groom::CURVEU.into(), curveu);
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_pass_through_or_near_their_points() {
        let mut set = CurveSettings { kind: 0, ..CurveSettings::defaults() };
        let poly = curve(&set).unwrap();
        assert_eq!(poly.vertices.len(), 4);
        let u = &poly.point_attributes[crate::groom::CURVEU];
        assert_eq!((u[0], u[3]), (0.0, 1.0));
        assert!(u.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(crate::groom::curves(&poly).len(), 1);

        // Smooth: through every point, `divisions` points a span.
        set.kind = 1;
        let smooth = curve(&set).unwrap();
        assert_eq!(smooth.vertices.len(), 3 * 12 + 1);
        for (k, p) in set.points.iter().enumerate() {
            assert!(smooth.vertices[k * 12].sub(Vec3::from_arr(*p)).length() < 1e-5, "passes through point {k}");
        }

        // B-spline: starts and ends on the end points, but cuts the middle ones' corners.
        set.kind = 2;
        let b = curve(&set).unwrap();
        assert!(b.vertices[0].sub(Vec3::from_arr(set.points[0])).length() < 1e-5);
        assert!(b.vertices.last().unwrap().sub(Vec3::from_arr(set.points[3])).length() < 1e-5);
        let nearest = b.vertices.iter().map(|v| v.sub(Vec3::from_arr(set.points[1])).length()).fold(f32::MAX, f32::min);
        assert!(nearest > 0.05, "doesn't pass through point 1");

        // Closed: ends where it starts.
        set.closed = true;
        for kind in 0..3 {
            set.kind = kind;
            let c = curve(&set).unwrap();
            assert!(c.vertices[0].sub(*c.vertices.last().unwrap()).length() < 1e-5, "type {kind} closes");
        }
        assert!(curve(&CurveSettings { points: vec![[0.0; 3]], ..CurveSettings::defaults() }).is_err());
        assert_eq!(CurveSettings::from_params(&set.params()), set);
    }
}
