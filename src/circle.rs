// Circle: a circle (or an ellipse, an arc or a slice of one) as one polygon, or as a curve,
// as Houdini's Circle makes it. In the XY, YZ or ZX plane, facing along the plane's third
// axis, with a radius on each of its two axes.
//
// Arcs run from Begin to End degrees, counterclockwise looking down the facing axis (0° is
// along the plane's first axis). Open arc: just the arc (a polygon closes it with a
// straight edge). Sliced arc: a slice of pie, through the centre.

use super::{Mesh, Vec3};

pub const TYPES: [&str; 2] = ["Polygon", "Curve"];
pub const PLANES: [&str; 3] = ["XY", "YZ", "ZX"];
pub const ARCS: [&str; 3] = ["Closed", "Open arc", "Sliced arc"];

#[derive(Clone, Debug, PartialEq)]
pub struct CircleSettings {
    pub kind: usize,
    pub plane: usize,
    pub radius: [f32; 2],
    pub center: [f32; 3],
    pub divisions: usize,
    pub arc: usize,
    pub angles: [f32; 2],
}

impl CircleSettings {
    pub fn defaults() -> CircleSettings {
        CircleSettings { kind: 0, plane: 2, radius: [1.0, 1.0], center: [0.0; 3], divisions: 24, arc: 0, angles: [0.0, 360.0] }
    }

    pub fn params(&self) -> Vec<f32> {
        let [r0, r1] = self.radius;
        let [cx, cy, cz] = self.center;
        vec![self.kind as f32, self.plane as f32, r0, r1, cx, cy, cz, self.divisions as f32, self.arc as f32, self.angles[0], self.angles[1]]
    }

    pub fn from_params(p: &[f32]) -> CircleSettings {
        let d = CircleSettings::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        CircleSettings {
            kind: (g(0, 0.0).max(0.0) as usize).min(TYPES.len() - 1),
            plane: (g(1, 2.0).max(0.0) as usize).min(PLANES.len() - 1),
            radius: [g(2, 1.0), g(3, 1.0)],
            center: [g(4, 0.0), g(5, 0.0), g(6, 0.0)],
            divisions: (g(7, d.divisions as f32).max(3.0) as usize).min(10_000),
            arc: (g(8, 0.0).max(0.0) as usize).min(ARCS.len() - 1),
            angles: [g(9, 0.0), g(10, 360.0)],
        }
    }
}

pub fn circle(set: &CircleSettings) -> Mesh {
    let divisions = set.divisions.max(3);
    // The plane's two axes (u, v) and the axis it faces (u × v).
    let (u, v) = match set.plane {
        0 => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        1 => (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
        _ => (Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0)),
    };
    let centre = Vec3::from_arr(set.center);
    let at = |degrees: f32| {
        let (s, c) = degrees.to_radians().sin_cos();
        centre.add(u.scale(c * set.radius[0])).add(v.scale(s * set.radius[1]))
    };
    let full = set.arc == 0;
    let (begin, end) = if full { (0.0, 360.0) } else { (set.angles[0], set.angles[1]) };
    // Points round the rim: a closed circle doesn't repeat its first one.
    let rim: Vec<Vec3> = if full {
        (0..divisions).map(|k| at(360.0 * k as f32 / divisions as f32)).collect()
    } else {
        (0..=divisions).map(|k| at(begin + (end - begin) * k as f32 / divisions as f32)).collect()
    };
    let mut mesh = Mesh::default();
    if set.kind == 1 {
        // A curve: the rim (closed: back to its start), and for a slice, in from and back
        // out to the centre.
        let mut points = rim.clone();
        if full {
            points.push(rim[0]);
        } else if set.arc == 2 {
            points.insert(0, centre);
            points.push(centre);
        }
        let mut along = vec![0.0f32];
        for w in points.windows(2) {
            along.push(along.last().unwrap() + w[1].sub(w[0]).length());
        }
        let total = along.last().copied().unwrap_or(0.0).max(1e-12);
        mesh.point_attributes.insert(crate::groom::CURVEU.into(), along.iter().map(|a| a / total).collect());
        mesh.vertices = points;
        return mesh;
    }
    // One polygon: a fan from its first point, or from the centre for a slice. Reversed
    // arcs (End before Begin) are turned round so the polygon still faces the same way.
    let mut outline = rim;
    if set.arc == 2 {
        outline.insert(0, centre);
    }
    if !full && end < begin {
        outline.reverse();
    }
    let n = outline.len();
    mesh.vertices = outline;
    mesh.indices = (1..n - 1).map(|k| [0, k, k + 1]).collect();
    let triangles = mesh.indices.len();
    if triangles > 1 {
        crate::poly::tag(&mut mesh, 0, vec![triangles]);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facing(m: &Mesh) -> Vec3 {
        m.indices.iter().fold(Vec3::zero(), |s, t| {
            let [a, b, c] = t.map(|k| m.vertices[k]);
            s.add(b.sub(a).cross(c.sub(a)))
        })
    }

    #[test]
    fn circles_arcs_and_slices() {
        // A full ZX circle: one polygon, its points on the rim, facing +Y.
        let c = circle(&CircleSettings::defaults());
        assert_eq!(c.vertices.len(), 24);
        assert_eq!(crate::poly::count(&c), 1);
        assert!(c.vertices.iter().all(|p| (p.length() - 1.0).abs() < 1e-5 && p.y.abs() < 1e-6));
        assert!(facing(&c).normalize().y > 0.999);
        // Area close to pi.
        let area = facing(&c).length() * 0.5;
        assert!((area - std::f32::consts::PI).abs() < 0.04, "{area}");
        // XY and YZ face +Z and +X; an ellipse has its radii.
        let xy = circle(&CircleSettings { plane: 0, radius: [2.0, 0.5], ..CircleSettings::defaults() });
        assert!(facing(&xy).normalize().z > 0.999);
        let (lo, hi) = xy.bounds().unwrap();
        assert!((hi.x - 2.0).abs() < 1e-5 && (lo.y + 0.5).abs() < 0.01);
        assert!(facing(&circle(&CircleSettings { plane: 1, ..CircleSettings::defaults() })).normalize().x > 0.999);
        // A quarter slice: the centre plus the arc's points, a quarter of the area.
        let slice = circle(&CircleSettings { arc: 2, angles: [0.0, 90.0], divisions: 32, ..CircleSettings::defaults() });
        assert_eq!(slice.vertices.len(), 34);
        assert!((facing(&slice).length() * 0.5 - std::f32::consts::PI / 4.0).abs() < 0.01);
        assert!(facing(&circle(&CircleSettings { arc: 2, angles: [90.0, 0.0], ..CircleSettings::defaults() })).normalize().y > 0.999, "reversed arcs still face up");
        // As a curve: closed, its last point back at its first; an open arc ends where it ends.
        let ring = circle(&CircleSettings { kind: 1, ..CircleSettings::defaults() });
        assert!(ring.indices.is_empty() && ring.vertices[0] == *ring.vertices.last().unwrap());
        assert_eq!(crate::groom::curves(&ring).len(), 1);
        let arc = circle(&CircleSettings { kind: 1, arc: 1, angles: [0.0, 180.0], center: [0.0, 1.0, 0.0], ..CircleSettings::defaults() });
        assert!(arc.vertices.last().unwrap().sub(Vec3::new(0.0, 1.0, -1.0)).length() < 1e-5 || arc.vertices.last().unwrap().sub(Vec3::new(0.0, 1.0, 1.0)).length() < 1e-5);
        assert_eq!(CircleSettings::from_params(&arc_settings().params()), arc_settings());
    }

    fn arc_settings() -> CircleSettings {
        CircleSettings { kind: 1, plane: 0, radius: [2.0, 3.0], center: [1.0, 2.0, 3.0], divisions: 7, arc: 2, angles: [10.0, 200.0] }
    }
}
