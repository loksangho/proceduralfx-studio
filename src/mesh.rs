// Math & mesh: moved out of main.rs.

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
    pub fn from_arr(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }
    pub fn add(self, o: Vec3) -> Vec3 {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: Vec3) -> Vec3 {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(self, s: f32) -> Vec3 {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
    pub fn mul(self, o: Vec3) -> Vec3 {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len > 1e-6 {
            self.scale(1.0 / len)
        } else {
            Vec3::zero()
        }
    }
    /// Rotate by Euler angles in degrees (X, then Y, then Z).
    pub fn rotate_deg(self, r: [f32; 3]) -> Vec3 {
        let (sx, cx) = r[0].to_radians().sin_cos();
        let (sy, cy) = r[1].to_radians().sin_cos();
        let (sz, cz) = r[2].to_radians().sin_cos();
        let v = Vec3::new(self.x, self.y * cx - self.z * sx, self.y * sx + self.z * cx);
        let v = Vec3::new(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
        Vec3::new(v.x * cz - v.y * sz, v.x * sz + v.y * cz, v.z)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<[usize; 3]>,
    pub point_attributes: HashMap<String, Vec<f32>>,
}

impl Mesh {
    // Adds sequential id / prim:id attributes, keeping ids a node already set
    // (for example i@id from a wrangle) when they match the element count.
    pub(crate) fn ensure_builtin_ids(&mut self) {
        let (points, prims) = (self.vertices.len(), self.indices.len());
        for (key, count) in [("id", points), ("prim:id", prims)] {
            if self.point_attributes.get(key).map_or(true, |values| values.len() != count) {
                self.point_attributes.insert(key.into(), (0..count).map(|index| index as f32).collect());
            }
        }
    }

    pub fn compute_normals(&self) -> Vec<Vec3> {
        let mut normals = vec![Vec3::zero(); self.vertices.len()];
        for tri in &self.indices {
            let v0 = self.vertices[tri[0]];
            let v1 = self.vertices[tri[1]];
            let v2 = self.vertices[tri[2]];
            let n = v1.sub(v0).cross(v2.sub(v0));
            for &i in tri {
                normals[i] = normals[i].add(n);
            }
        }
        normals.into_iter().map(|n| n.normalize()).collect()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.vertices.first()?;
        let (mut lo, mut hi) = (first, first);
        for v in &self.vertices {
            lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
            hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        }
        Some((lo, hi))
    }

    pub fn validation_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let invalid_indices = self.indices.iter().filter(|tri| {
            tri.iter().any(|&i| i >= self.vertices.len())
        }).count();
        if invalid_indices > 0 {
            warnings.push(format!("{invalid_indices} triangles have invalid vertex indices"));
        }
        let non_finite = self.vertices.iter().filter(|v| {
            !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite()
        }).count();
        if non_finite > 0 {
            warnings.push(format!("{non_finite} vertices contain non-finite coordinates"));
        }
        let degenerate = self.indices.iter().filter(|tri| {
            let Some((&a, rest)) = tri.split_first() else { return true; };
            let [b, c] = [rest[0], rest[1]];
            if a >= self.vertices.len() || b >= self.vertices.len() || c >= self.vertices.len() {
                return false;
            }
            self.vertices[b].sub(self.vertices[a]).cross(self.vertices[c].sub(self.vertices[a])).length()
                <= 1e-8
        }).count();
        if degenerate > 0 {
            warnings.push(format!("{degenerate} degenerate triangles"));
        }
        warnings
    }

    pub fn export_stl(&self, filename: &str) -> std::io::Result<()> {
        let mut f = std::io::BufWriter::new(File::create(filename)?);
        writeln!(f, "solid proceduralfx_studio")?;
        for t in &self.indices {
            let (a, b, c) = (self.vertices[t[0]], self.vertices[t[1]], self.vertices[t[2]]);
            let n = b.sub(a).cross(c.sub(a)).normalize();
            writeln!(f, "facet normal {} {} {}\n outer loop", n.x, n.y, n.z)?;
            for v in [a, b, c] {
                writeln!(f, "  vertex {} {} {}", v.x, v.y, v.z)?;
            }
            writeln!(f, " endloop\nendfacet")?;
        }
        writeln!(f, "endsolid proceduralfx_studio")
    }

    pub fn export_obj(&self, filename: &str) -> std::io::Result<()> {
        let mut file = File::create(filename)?;
        writeln!(file, "# ProceduralFX Studio Export")?;
        for v in &self.vertices {
            writeln!(file, "v {:.6} {:.6} {:.6}", v.x, v.y, v.z)?;
        }
        for n in &self.compute_normals() {
            writeln!(file, "vn {:.6} {:.6} {:.6}", n.x, n.y, n.z)?;
        }
        for tri in &self.indices {
            let (a, b, c) = (tri[0] + 1, tri[1] + 1, tri[2] + 1);
            writeln!(file, "f {a}//{a} {b}//{b} {c}//{c}")?;
        }
        Ok(())
    }
}
