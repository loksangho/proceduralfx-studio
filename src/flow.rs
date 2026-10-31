// Flow control in geometry networks: For-Each loops and Compile blocks.
//
// A For-Each loop is a Begin/End pair, like Houdini's Block Begin / Block End:
//   input -> For-Each Begin -> (the loop body) -> For-Each End
// The End node runs the body once per iteration, with the Begin node putting
// out that iteration's geometry, and merges the results (or, for feedback
// loops, passes each result to the next iteration). The variants:
// - Connected Piece: each connected part of the mesh.
// - Named Primitive: each value of an attribute (`name`, `piece`, `class`…).
// - Point / Primitive: each point or primitive on its own.
// - Number: the whole input, N times, with `value` counting up.
// - Feedback: N iterations, each starting from the previous one's result.
// Begin adds detail attributes `iteration`, `numiterations` and `value`.
//
// A Compile Block (Compile Begin / Compile End) marks a part of the network
// to cook as a unit: its result is cached, and For-Each loops inside it run
// their iterations on several threads.

use super::Mesh;

pub const MODES: [&str; 6] = ["Connected Piece", "Named Primitive", "Point", "Primitive", "Number", "Feedback"];
pub const CONNECTED: usize = 0;
pub const NAMED: usize = 1;
pub const POINT: usize = 2;
pub const PRIMITIVE: usize = 3;
pub const NUMBER: usize = 4;
pub const FEEDBACK: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub struct ForEachSettings {
    pub mode: usize,
    // Named Primitive: the attribute whose values make the pieces.
    pub attribute: String,
    // Number and Feedback.
    pub iterations: usize,
    pub start: f32,
    pub increment: f32,
    // Runs only this iteration (-1: all), for checking one piece.
    pub single_pass: i32,
    // Inside a Compile Block, run iterations on several threads.
    pub multithread: bool,
    // The paired For-Each Begin node.
    pub begin: Option<usize>,
}

impl ForEachSettings {
    pub fn new(mode: usize) -> Self {
        ForEachSettings {
            mode,
            attribute: if mode == NAMED { "name".into() } else { String::new() },
            iterations: 10,
            start: 0.0,
            increment: 1.0,
            single_pass: -1,
            multithread: true,
            begin: None,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![
            self.mode as f32,
            self.iterations as f32,
            self.start,
            self.increment,
            self.single_pass as f32,
            self.multithread as u8 as f32,
            self.begin.map_or(-1.0, |b| b as f32),
        ]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let g = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
        ForEachSettings {
            mode: (g(0, 0.0).max(0.0) as usize).min(MODES.len() - 1),
            attribute: String::new(),
            iterations: g(1, 10.0).clamp(0.0, 1e6) as usize,
            start: g(2, 0.0),
            increment: g(3, 1.0),
            single_pass: g(4, -1.0) as i32,
            multithread: g(5, 1.0) > 0.5,
            begin: (g(6, -1.0) >= 0.0).then(|| g(6, -1.0) as usize),
        }
    }

    pub fn menu_name(&self) -> String {
        match self.mode {
            FEEDBACK => "For Loop with Feedback".into(),
            m => format!("For-Each {}", MODES[m]),
        }
    }
}

// Attribute classes by name prefix.
fn class(key: &str) -> u8 {
    if key.starts_with("prim:") {
        1
    } else if key.starts_with("vertex:") {
        2
    } else if key.starts_with("detail:") {
        3
    } else {
        0
    }
}

// The primitives `prims` (with the points they use) as a mesh of their own.
pub fn extract_prims(mesh: &Mesh, prims: &[usize]) -> Mesh {
    let mut remap = vec![usize::MAX; mesh.vertices.len()];
    let mut used = Vec::new();
    let mut out = Mesh::default();
    for &t in prims {
        let tri = mesh.indices[t];
        out.indices.push(tri.map(|v| {
            if remap[v] == usize::MAX {
                remap[v] = used.len();
                used.push(v);
            }
            remap[v]
        }));
    }
    out.vertices = used.iter().map(|&v| mesh.vertices[v]).collect();
    let (np, nt) = (mesh.vertices.len(), mesh.indices.len());
    for (key, values) in &mesh.point_attributes {
        let picked: Option<Vec<f32>> = match class(key) {
            0 if np > 0 && values.len() % np == 0 => {
                let w = values.len() / np;
                Some(used.iter().flat_map(|&v| values[v * w..v * w + w].iter().copied()).collect())
            }
            1 if nt > 0 && values.len() % nt == 0 => {
                let w = values.len() / nt;
                Some(prims.iter().flat_map(|&t| values[t * w..t * w + w].iter().copied()).collect())
            }
            2 if nt > 0 && values.len() % (nt * 3) == 0 => {
                let w = values.len() / (nt * 3);
                Some(prims.iter().flat_map(|&t| values[t * 3 * w..(t * 3 + 3) * w].iter().copied()).collect())
            }
            3 => Some(values.clone()),
            _ => None,
        };
        if let Some(v) = picked {
            out.point_attributes.insert(key.clone(), v);
        }
    }
    out
}

// The points `points` alone (no primitives).
pub fn extract_points(mesh: &Mesh, points: &[usize]) -> Mesh {
    let mut out = Mesh { vertices: points.iter().map(|&p| mesh.vertices[p]).collect(), ..Mesh::default() };
    let np = mesh.vertices.len();
    for (key, values) in &mesh.point_attributes {
        match class(key) {
            0 if np > 0 && values.len() % np == 0 => {
                let w = values.len() / np;
                out.point_attributes.insert(key.clone(), points.iter().flat_map(|&p| values[p * w..p * w + w].iter().copied()).collect());
            }
            3 => {
                out.point_attributes.insert(key.clone(), values.clone());
            }
            _ => {}
        }
    }
    out
}

// Primitives grouped into connected parts (sharing points); in first-seen order.
fn connected_prims(mesh: &Mesh) -> Vec<Vec<usize>> {
    let n = mesh.vertices.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for t in &mesh.indices {
        for k in 1..3 {
            let (a, b) = (find(&mut parent, t[0]), find(&mut parent, t[k]));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut order: Vec<usize> = Vec::new();
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, t) in mesh.indices.iter().enumerate() {
        let root = find(&mut parent, t[0]);
        groups.entry(root).or_insert_with(|| {
            order.push(root);
            Vec::new()
        }).push(i);
    }
    order.into_iter().map(|r| groups.remove(&r).unwrap()).collect()
}

// The pieces made by each value of an attribute (`name`, `piece`, `variant`…), in the
// order they first appear, each with its value: primitives by their primitive
// attribute (or their first point's), or points alone when there are no primitives.
pub fn named_pieces(mesh: &Mesh, name: &str) -> Result<Vec<(f32, Mesh)>, String> {
    let (np, nt) = (mesh.vertices.len(), mesh.indices.len());
    let name = name.trim();
    if name.is_empty() {
        return Err("enter the attribute whose values make the pieces (name, piece, class…)".into());
    }
    let key = |v: f32| (v * 1000.0).round() as i64;
    let prim = mesh.point_attributes.get(&format!("prim:{name}")).filter(|v| nt > 0 && v.len() == nt);
    let point = mesh.point_attributes.get(name).filter(|v| np > 0 && v.len() == np);
    let (values, elements): (Vec<f32>, usize) = match (prim, point) {
        (Some(v), _) => (v.clone(), nt),
        (None, Some(v)) if nt > 0 => (mesh.indices.iter().map(|t| v[t[0]]).collect(), nt),
        (None, Some(v)) => (v.clone(), np),
        _ => return Err(format!("the input has no `{name}` attribute (primitive or point)")),
    };
    let mut order = Vec::new();
    let mut groups: std::collections::HashMap<i64, Vec<usize>> = Default::default();
    for (i, &v) in values.iter().enumerate().take(elements) {
        groups.entry(key(v)).or_insert_with(|| {
            order.push(v);
            Vec::new()
        }).push(i);
    }
    Ok(order
        .into_iter()
        .map(|v| {
            let members = &groups[&key(v)];
            (v, if nt > 0 { extract_prims(mesh, members) } else { extract_points(mesh, members) })
        })
        .collect())
}

// The pieces a piece-based loop runs over.
pub fn pieces(mesh: &Mesh, set: &ForEachSettings) -> Result<Vec<Mesh>, String> {
    let (np, nt) = (mesh.vertices.len(), mesh.indices.len());
    Ok(match set.mode {
        POINT => (0..np).map(|p| extract_points(mesh, &[p])).collect(),
        PRIMITIVE => (0..nt).map(|t| extract_prims(mesh, &[t])).collect(),
        NAMED => named_pieces(mesh, &set.attribute)?.into_iter().map(|(_, piece)| piece).collect(),
        _ => {
            if nt == 0 {
                // Loose points are each their own piece.
                (0..np).map(|p| extract_points(mesh, &[p])).collect()
            } else {
                connected_prims(mesh).iter().map(|prims| extract_prims(mesh, prims)).collect()
            }
        }
    })
}

// Adds the loop's metadata to an iteration's geometry.
pub fn stamp(mut mesh: Mesh, iteration: usize, count: usize, value: f32) -> Mesh {
    mesh.point_attributes.insert("detail:iteration".into(), vec![iteration as f32]);
    mesh.point_attributes.insert("detail:numiterations".into(), vec![count as f32]);
    mesh.point_attributes.insert("detail:value".into(), vec![value]);
    mesh
}

// Compile End results, keyed by everything upstream (and the time when it matters).
static COMPILED: std::sync::Mutex<Vec<(u64, Mesh)>> = std::sync::Mutex::new(Vec::new());

pub fn compiled(key: u64) -> Option<Mesh> {
    COMPILED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, m)| m.clone())
}

pub fn store_compiled(key: u64, mesh: &Mesh) {
    let mut cache = COMPILED.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(k, _)| *k != key);
    cache.push((key, mesh.clone()));
    if cache.len() > 16 {
        cache.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    // Two separate triangles with a prim attribute and a point attribute.
    fn two_triangles() -> Mesh {
        let mut m = Mesh::default();
        for x in [0.0, 5.0] {
            m.vertices.extend([Vec3::new(x, 0.0, 0.0), Vec3::new(x + 1.0, 0.0, 0.0), Vec3::new(x, 1.0, 0.0)]);
        }
        m.indices = vec![[0, 1, 2], [3, 4, 5]];
        m.point_attributes.insert("prim:name".into(), vec![7.0, 9.0]);
        m.point_attributes.insert("Cd".into(), (0..18).map(|i| i as f32).collect());
        m.point_attributes.insert("vertex:uv".into(), (0..12).map(|i| i as f32).collect());
        m.point_attributes.insert("detail:tag".into(), vec![3.0]);
        m
    }

    #[test]
    fn pieces_keep_their_attributes() {
        let m = two_triangles();
        let parts = pieces(&m, &ForEachSettings::new(CONNECTED)).unwrap();
        assert_eq!(parts.len(), 2);
        let second = &parts[1];
        assert_eq!(second.vertices[0], Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(second.indices, vec![[0, 1, 2]]);
        assert_eq!(second.point_attributes["Cd"], (9..18).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(second.point_attributes["prim:name"], vec![9.0]);
        assert_eq!(second.point_attributes["vertex:uv"], (6..12).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(second.point_attributes["detail:tag"], vec![3.0]);
        let named = pieces(&m, &ForEachSettings::new(NAMED)).unwrap();
        assert_eq!(named.len(), 2);
        assert_eq!(pieces(&m, &ForEachSettings::new(POINT)).unwrap().len(), 6);
        assert_eq!(pieces(&m, &ForEachSettings::new(PRIMITIVE)).unwrap().len(), 2);
        assert!(pieces(&m, &ForEachSettings { attribute: "nope".into(), ..ForEachSettings::new(NAMED) }).is_err());
        let s = ForEachSettings { iterations: 4, begin: Some(12), single_pass: 2, ..ForEachSettings::new(FEEDBACK) };
        assert_eq!(ForEachSettings { attribute: String::new(), ..s.clone() }, ForEachSettings::from_params(&s.to_params()));
        let stamped = stamp(Mesh::default(), 2, 5, 0.5);
        assert_eq!(stamped.point_attributes["detail:iteration"], vec![2.0]);
    }
}
