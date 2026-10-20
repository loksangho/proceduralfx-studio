// Groups, as in Houdini: named sets of points or primitives, stored on the
// geometry as 0/1 attributes (`group:name` for points, `prim:group:name` for
// primitives), and the group patterns nodes use to choose what they affect.
//
// Pattern syntax (space-separated, applied left to right):
//   name, side*       groups by name or wildcard
//   *                 everything
//   0-10  20-40:2  7  element numbers and ranges (optional step)
//   @P.y>0  @Cd.x==1  @id=5  @ptnum<10   ad-hoc expressions (no spaces inside)
//   ^token            remove that token's elements (a leading ^ starts from everything)
//   !token            add everything *not* in that token

use super::{hash01, poly, Mesh, Vec3};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupDomain {
    Points,
    Prims,
}

pub const GROUP_TYPES: [&str; 3] = ["Guess from group", "Points", "Primitives"];

// The group a node applies to (stored on the node, saved with the scene).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeGroup {
    pub pattern: String,
    // Index into GROUP_TYPES.
    pub kind: u8,
}

impl NodeGroup {
    pub fn is_empty(&self) -> bool {
        self.pattern.trim().is_empty()
    }
}

pub fn key(name: &str, domain: GroupDomain) -> String {
    match domain {
        GroupDomain::Points => format!("group:{name}"),
        GroupDomain::Prims => format!("prim:group:{name}"),
    }
}

// Point and primitive group names on `mesh`.
pub fn names(mesh: &Mesh) -> (Vec<String>, Vec<String>) {
    let (mut points, mut prims) = (Vec::new(), Vec::new());
    for (k, values) in &mesh.point_attributes {
        if let Some(name) = k.strip_prefix("prim:group:") {
            if values.len() == mesh.indices.len() {
                prims.push(name.to_owned());
            }
        } else if let Some(name) = k.strip_prefix("group:") {
            if values.len() == mesh.vertices.len() {
                points.push(name.to_owned());
            }
        }
    }
    points.sort();
    prims.sort();
    (points, prims)
}

fn wildcard(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => name.len() >= head.len() && name.starts_with(head) && {
            let rest = &name[head.len()..];
            (0..=rest.len()).any(|i| wildcard(tail, &rest[i..]))
        },
    }
}

// Which domain a pattern is about when the node doesn't say: primitives if it
// names only primitive groups, otherwise points.
pub fn guess_domain(mesh: &Mesh, pattern: &str) -> GroupDomain {
    let (points, prims) = names(mesh);
    let named: Vec<&str> = pattern
        .split_whitespace()
        .map(|t| t.trim_start_matches(['^', '!']))
        .filter(|t| !t.is_empty() && !t.starts_with('@') && !t.starts_with(|c: char| c.is_ascii_digit()) && *t != "*")
        .collect();
    let hits = |list: &[String]| named.iter().any(|t| list.iter().any(|n| wildcard(t, n)));
    if !named.is_empty() && hits(&prims) && !hits(&points) {
        GroupDomain::Prims
    } else if pattern.contains("@primnum") {
        GroupDomain::Prims
    } else {
        GroupDomain::Points
    }
}

// Primitive patterns count polygons: `polys` are their triangles (None: one per triangle).
fn element_value(mesh: &Mesh, domain: GroupDomain, attr: &str, i: usize, polys: Option<&[Range<usize>]>) -> Option<f32> {
    let (name, component) = match attr.split_once('.') {
        Some((n, c)) => (n, match c {
            "x" | "r" | "u" => 0,
            "y" | "g" | "v" => 1,
            "z" | "b" | "w" => 2,
            _ => return None,
        }),
        None => match attr.split_once('[') {
            Some((n, rest)) => (n, rest.trim_end_matches(']').parse().ok()?),
            None => (attr, 0),
        },
    };
    let pick = |v: Vec3| [v.x, v.y, v.z][component];
    if let (GroupDomain::Prims, Some(polys)) = (domain, polys) {
        let run = &polys[i];
        match name {
            "primnum" => return Some(i as f32),
            "P" => return Some(pick(poly::centroid(mesh, run))),
            "N" => return Some(pick(poly::normal(mesh, run))),
            _ => return element_value(mesh, domain, attr, run.start, None),
        }
    }
    match (domain, name) {
        (GroupDomain::Points, "ptnum") | (GroupDomain::Prims, "primnum") => return Some(i as f32),
        (GroupDomain::Points, "P") => return Some(pick(mesh.vertices[i])),
        (GroupDomain::Prims, "P") => {
            let [a, b, c] = mesh.indices[i].map(|v| mesh.vertices[v]);
            return Some(pick(a.add(b).add(c).scale(1.0 / 3.0)));
        }
        (GroupDomain::Prims, "N") => {
            let [a, b, c] = mesh.indices[i].map(|v| mesh.vertices[v]);
            let n = b.sub(a).cross(c.sub(a));
            return Some(pick(if n.length() > 1e-20 { n.normalize() } else { n }));
        }
        _ => {}
    }
    let count = if domain == GroupDomain::Points { mesh.vertices.len() } else { mesh.indices.len() };
    let key = match domain {
        GroupDomain::Points => name.to_owned(),
        GroupDomain::Prims => format!("prim:{name}"),
    };
    if domain == GroupDomain::Points && name == "N" && !mesh.point_attributes.contains_key("N") {
        return Some(pick(mesh.compute_normals()[i]));
    }
    let values = mesh.point_attributes.get(&key)?;
    let width = values.len().checked_div(count).filter(|w| *w > 0)?;
    values.get(i * width + component.min(width - 1)).copied()
}

fn expression(mesh: &Mesh, domain: GroupDomain, token: &str, count: usize, polys: Option<&[Range<usize>]>) -> Result<Vec<bool>, String> {
    let body = &token[1..];
    let ops = ["==", "!=", ">=", "<=", ">", "<", "="];
    let (at, op) = ops
        .iter()
        .filter_map(|op| body.find(op).map(|i| (i, *op)))
        .min_by_key(|(i, op)| (*i, std::cmp::Reverse(op.len())))
        .ok_or_else(|| format!("'{token}': expected a comparison like @P.y>0"))?;
    let (attr, value) = (&body[..at], &body[at + op.len()..]);
    let value: f32 = value.trim().parse().map_err(|_| format!("'{token}': '{value}' is not a number"))?;
    let mut out = vec![false; count];
    for (i, slot) in out.iter_mut().enumerate() {
        let Some(v) = element_value(mesh, domain, attr, i, polys) else {
            return Err(format!("'{token}': no attribute '{attr}' on the {}", if domain == GroupDomain::Points { "points" } else { "primitives" }));
        };
        *slot = match op {
            "==" | "=" => (v - value).abs() <= 1e-5,
            "!=" => (v - value).abs() > 1e-5,
            ">=" => v >= value,
            "<=" => v <= value,
            ">" => v > value,
            _ => v < value,
        };
    }
    Ok(out)
}

fn token_set(mesh: &Mesh, domain: GroupDomain, token: &str, count: usize, polys: Option<&[Range<usize>]>) -> Result<Vec<bool>, String> {
    if token == "*" {
        return Ok(vec![true; count]);
    }
    if token.starts_with('@') {
        return expression(mesh, domain, token, count, polys);
    }
    if token.starts_with(|c: char| c.is_ascii_digit()) {
        let (range, step) = match token.split_once(':') {
            Some((r, s)) => (r, s.parse::<usize>().map_err(|_| format!("'{token}': bad step"))?.max(1)),
            None => (token, 1),
        };
        let (lo, hi) = match range.split_once('-') {
            Some((a, b)) => (a.parse::<usize>(), b.parse::<usize>()),
            None => (range.parse::<usize>(), range.parse::<usize>()),
        };
        let (Ok(lo), Ok(hi)) = (lo, hi) else { return Err(format!("'{token}' is not a number or range")) };
        let mut out = vec![false; count];
        for i in (lo..=hi.min(count.saturating_sub(1))).step_by(step) {
            out[i] = true;
        }
        return Ok(out);
    }
    let (points, prims) = names(mesh);
    let list = if domain == GroupDomain::Points { points } else { prims };
    let matched: Vec<&String> = list.iter().filter(|n| wildcard(token, n)).collect();
    if matched.is_empty() && !token.contains('*') {
        return Err(format!("no {} group named '{token}'", if domain == GroupDomain::Points { "point" } else { "primitive" }));
    }
    let triangles = if polys.is_some() { mesh.indices.len() } else { count };
    let mut out = vec![false; triangles];
    for name in matched {
        if let Some(values) = mesh.point_attributes.get(&key(name, domain)) {
            for (slot, v) in out.iter_mut().zip(values) {
                *slot |= *v > 0.5;
            }
        }
    }
    // A polygon is in a group when any of its triangles is.
    Ok(match polys {
        Some(polys) => poly::collapse(polys, &out),
        None => out,
    })
}

// Membership of each point (or primitive) in `pattern`.
// Primitives are polygons here (numbers, @primnum, @P), and the result is per triangle.
pub fn evaluate(mesh: &Mesh, pattern: &str, domain: GroupDomain) -> Result<Vec<bool>, String> {
    let polys = (domain == GroupDomain::Prims && poly::has_polygons(mesh)).then(|| poly::runs(mesh));
    let polys = polys.as_deref();
    let count = match (domain, polys) {
        (GroupDomain::Points, _) => mesh.vertices.len(),
        (GroupDomain::Prims, Some(polys)) => polys.len(),
        (GroupDomain::Prims, None) => mesh.indices.len(),
    };
    let tokens: Vec<&str> = pattern.split_whitespace().collect();
    let mut set = vec![tokens.first().is_some_and(|t| t.starts_with('^')); count];
    for token in tokens {
        if let Some(rest) = token.strip_prefix('^') {
            for (s, r) in set.iter_mut().zip(token_set(mesh, domain, rest, count, polys)?) {
                *s &= !r;
            }
        } else if let Some(rest) = token.strip_prefix('!') {
            for (s, r) in set.iter_mut().zip(token_set(mesh, domain, rest, count, polys)?) {
                *s |= !r;
            }
        } else {
            for (s, r) in set.iter_mut().zip(token_set(mesh, domain, token, count, polys)?) {
                *s |= r;
            }
        }
    }
    Ok(match polys {
        Some(polys) => poly::expand(polys, &set, mesh.indices.len()),
        None => set,
    })
}

fn resolve_domain(mesh: &Mesh, group: &NodeGroup) -> GroupDomain {
    match group.kind {
        1 => GroupDomain::Points,
        2 => GroupDomain::Prims,
        _ => guess_domain(mesh, &group.pattern),
    }
}

// Points the node's group covers (a primitive group covers its primitives' points).
// None when the node has no group: everything.
pub fn point_mask(mesh: &Mesh, group: &NodeGroup) -> Result<Option<Vec<bool>>, String> {
    if group.is_empty() {
        return Ok(None);
    }
    Ok(Some(match resolve_domain(mesh, group) {
        GroupDomain::Points => evaluate(mesh, &group.pattern, GroupDomain::Points)?,
        GroupDomain::Prims => {
            let prims = evaluate(mesh, &group.pattern, GroupDomain::Prims)?;
            let mut points = vec![false; mesh.vertices.len()];
            for (tri, inside) in mesh.indices.iter().zip(prims) {
                if inside {
                    for &v in tri {
                        points[v] = true;
                    }
                }
            }
            points
        }
    }))
}

// Primitives the node's group covers (a point group covers primitives whose points are all in it).
pub fn prim_mask(mesh: &Mesh, group: &NodeGroup) -> Result<Option<Vec<bool>>, String> {
    if group.is_empty() {
        return Ok(None);
    }
    Ok(Some(match resolve_domain(mesh, group) {
        GroupDomain::Prims => evaluate(mesh, &group.pattern, GroupDomain::Prims)?,
        GroupDomain::Points => {
            let points = evaluate(mesh, &group.pattern, GroupDomain::Points)?;
            mesh.indices.iter().map(|tri| tri.iter().all(|&v| points[v])).collect()
        }
    }))
}

// After a node ran on everything, puts back what it changed outside the group:
// positions and attributes of points (and attributes of primitives and their
// vertices) that aren't in it. New attributes read 0 outside the group.
// Returns false if the topology changed, when nothing can be restored.
pub fn restrict_to_group(input: &Mesh, output: &mut Mesh, points: &[bool], prims: &[bool]) -> bool {
    if output.vertices.len() != input.vertices.len() || output.indices != input.indices {
        return false;
    }
    for (i, inside) in points.iter().enumerate() {
        if !inside {
            output.vertices[i] = input.vertices[i];
        }
    }
    let (np, nt) = (input.vertices.len(), input.indices.len());
    for (key, values) in output.point_attributes.iter_mut() {
        let (count, mask): (usize, Box<dyn Fn(usize) -> bool>) = if key.starts_with("prim:") {
            (nt, Box::new(|e: usize| prims[e]))
        } else if key.starts_with("vertex:") {
            (nt * 3, Box::new(|e: usize| prims[e / 3]))
        } else if key.starts_with("detail:") {
            continue;
        } else {
            (np, Box::new(|e: usize| points[e]))
        };
        if count == 0 || values.len() % count != 0 {
            continue;
        }
        let width = values.len() / count;
        let original = input.point_attributes.get(key).filter(|v| v.len() == values.len());
        for e in 0..count {
            if !mask(e) {
                for c in 0..width {
                    values[e * width + c] = original.map_or(0.0, |o| o[e * width + c]);
                }
            }
        }
    }
    true
}

// Keeps only the primitives in `prims` (points are kept, so indices stay valid).
pub fn only_prims(mesh: &Mesh, prims: &[bool]) -> Mesh {
    let mut out = mesh.clone();
    let kept: Vec<usize> = (0..mesh.indices.len()).filter(|&t| prims[t]).collect();
    out.indices = kept.iter().map(|&t| mesh.indices[t]).collect();
    let nt = mesh.indices.len();
    for (key, values) in out.point_attributes.iter_mut() {
        let per = if key.starts_with("prim:") { 1 } else if key.starts_with("vertex:") { 3 } else { continue };
        if nt == 0 || values.len() % (nt * per) != 0 {
            continue;
        }
        let width = values.len() / (nt * per) * per;
        *values = kept.iter().flat_map(|&t| values[t * width..(t + 1) * width].to_vec()).collect();
    }
    out
}

// Element numbers as a compact pattern: runs become ranges ("4-9"), evenly
// spaced runs get a step ("0-20:2"), lone numbers stay as they are.
pub fn ranges(sorted: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        // The longest run from here with a constant step.
        let mut end = i;
        if i + 1 < sorted.len() {
            let step = sorted[i + 1] - start;
            while end + 1 < sorted.len() && sorted[end + 1] - sorted[end] == step {
                end += 1;
            }
            // Stepped runs need three numbers to be shorter than listing them.
            if step > 1 && end - i < 2 {
                end = i;
            }
            if end > i {
                let last = sorted[end];
                parts.push(if step == 1 { format!("{start}-{last}") } else { format!("{start}-{last}:{step}") });
                i = end + 1;
                continue;
            }
        }
        parts.push(start.to_string());
        i += 1;
    }
    parts.join(" ")
}

// The shortest group pattern for exactly these elements: `*`, an existing
// group's name, number ranges, or everything except some ranges.
// Selected primitives are triangles; the pattern numbers polygons.
pub fn describe(mesh: &Mesh, selected: &[usize], domain: GroupDomain) -> String {
    let polys = (domain == GroupDomain::Prims && poly::has_polygons(mesh)).then(|| poly::runs(mesh));
    let selected: Vec<usize> = match &polys {
        Some(_) => {
            let of = poly::of_triangles(mesh);
            selected.iter().filter_map(|t| of.get(*t).copied()).collect()
        }
        None => selected.to_vec(),
    };
    let count = match (&polys, domain) {
        (Some(polys), _) => polys.len(),
        (None, GroupDomain::Points) => mesh.vertices.len(),
        (None, GroupDomain::Prims) => mesh.indices.len(),
    };
    let mut chosen: Vec<usize> = selected.iter().copied().filter(|&i| i < count).collect();
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.is_empty() {
        return String::new();
    }
    if chosen.len() == count {
        return "*".into();
    }
    let mut members = vec![false; count];
    for &i in &chosen {
        members[i] = true;
    }
    let (points, prims) = names(mesh);
    for name in if domain == GroupDomain::Points { points } else { prims } {
        let named = evaluate(mesh, &name, domain).map(|m| match &polys {
            Some(polys) => poly::collapse(polys, &m),
            None => m,
        });
        if named.is_ok_and(|m| m == members) {
            return name;
        }
    }
    let direct = ranges(&chosen);
    let rest: Vec<usize> = (0..count).filter(|&i| !members[i]).collect();
    let inverse = format!("* ^{}", ranges(&rest).replace(' ', " ^"));
    if inverse.len() < direct.len() { inverse } else { direct }
}

// --------------------------------------------------------------------------
// Group Create
// --------------------------------------------------------------------------

pub const CREATE_METHODS: [&str; 4] = ["Bounding box", "Random", "Normal direction", "Expression"];
pub const COMBINE_MODES: [&str; 3] = ["Replace", "Union with existing", "Intersect with existing"];

#[derive(Clone, Debug, PartialEq)]
pub struct GroupCreateSettings {
    pub name: String,
    pub domain: GroupDomain,
    pub method: usize,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    // Random: fraction of elements kept.
    pub percent: f32,
    pub seed: u32,
    // Normal direction: elements facing within `spread` degrees of `direction`.
    pub direction: [f32; 3],
    pub spread: f32,
    // Expression: a group pattern, e.g. @P.y>0.
    pub expression: String,
    pub combine: usize,
    pub invert: bool,
}

pub fn group_create(input: &Mesh, set: &GroupCreateSettings) -> Result<Mesh, String> {
    let name = set.name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) || name.contains(':') || name.contains('*') {
        return Err("give the group a single-word name".into());
    }
    // Primitives are polygons: each is chosen as a whole, then stored on its triangles.
    let polys = poly::runs(input);
    let count = if set.domain == GroupDomain::Points { input.vertices.len() } else { polys.len() };
    let position = |i: usize| match set.domain {
        GroupDomain::Points => input.vertices[i],
        GroupDomain::Prims => poly::centroid(input, &polys[i]),
    };
    let mut members: Vec<bool> = match set.method {
        0 => (0..count)
            .map(|i| {
                let p = position(i);
                (0..3).all(|a| {
                    let (lo, hi) = (set.bbox_min[a].min(set.bbox_max[a]), set.bbox_min[a].max(set.bbox_max[a]));
                    let v = [p.x, p.y, p.z][a];
                    v >= lo && v <= hi
                })
            })
            .collect(),
        1 => (0..count).map(|i| hash01(i as u32, set.seed) < set.percent.clamp(0.0, 100.0) / 100.0).collect(),
        2 => {
            let d = Vec3::from_arr(set.direction);
            let d = if d.length() > 1e-9 { d.normalize() } else { Vec3::new(0.0, 1.0, 0.0) };
            let cos = set.spread.clamp(0.0, 180.0).to_radians().cos();
            let normals: Vec<Vec3> = match set.domain {
                GroupDomain::Points => input.compute_normals(),
                GroupDomain::Prims => polys.iter().map(|run| poly::normal(input, run)).collect(),
            };
            normals.iter().map(|n| n.dot(d) >= cos - 1e-6).collect()
        }
        _ => match set.domain {
            GroupDomain::Points => evaluate(input, &set.expression, set.domain)?,
            GroupDomain::Prims => poly::collapse(&polys, &evaluate(input, &set.expression, set.domain)?),
        },
    };
    if set.domain == GroupDomain::Prims {
        members = poly::expand(&polys, &members, input.indices.len());
    }
    let count = if set.domain == GroupDomain::Points { input.vertices.len() } else { input.indices.len() };
    if set.invert {
        for m in &mut members {
            *m = !*m;
        }
    }
    let mut out = input.clone();
    let k = key(name, set.domain);
    let existing = input.point_attributes.get(&k).filter(|v| v.len() == count);
    let values = members
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            let old = existing.is_some_and(|e| e[i] > 0.5);
            let keep = match set.combine {
                1 => m || old,
                2 => m && old,
                _ => m,
            };
            keep as u8 as f32
        })
        .collect();
    out.point_attributes.insert(k, values);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 4 x 4 grid of points (y = 0), two triangles per cell.
    fn grid() -> Mesh {
        let mut mesh = Mesh::default();
        for z in 0..4 {
            for x in 0..4 {
                mesh.vertices.push(Vec3::new(x as f32, 0.0, z as f32));
            }
        }
        for z in 0..3 {
            for x in 0..3 {
                let i = z * 4 + x;
                mesh.indices.push([i, i + 4, i + 1]);
                mesh.indices.push([i + 1, i + 4, i + 5]);
            }
        }
        mesh
    }

    fn create(mesh: &Mesh, name: &str, domain: GroupDomain, method: usize, f: impl FnOnce(&mut GroupCreateSettings)) -> Mesh {
        let mut set = GroupCreateSettings {
            name: name.into(),
            domain,
            method,
            bbox_min: [-0.5, -1.0, -0.5],
            bbox_max: [1.5, 1.0, 1.5],
            percent: 50.0,
            seed: 1,
            direction: [0.0, 1.0, 0.0],
            spread: 10.0,
            expression: String::new(),
            combine: 0,
            invert: false,
        };
        f(&mut set);
        group_create(mesh, &set).unwrap()
    }

    fn count(mask: &[bool]) -> usize {
        mask.iter().filter(|b| **b).count()
    }

    #[test]
    fn patterns_names_ranges_expressions_and_exclusions() {
        let g = create(&grid(), "corner", GroupDomain::Points, 0, |_| {});
        let g = create(&g, "row0", GroupDomain::Points, 3, |s| s.expression = "@P.z==0".into());
        assert_eq!(count(&evaluate(&g, "corner", GroupDomain::Points).unwrap()), 4);
        assert_eq!(count(&evaluate(&g, "row0", GroupDomain::Points).unwrap()), 4);
        assert_eq!(count(&evaluate(&g, "corner row0", GroupDomain::Points).unwrap()), 6, "union");
        assert_eq!(count(&evaluate(&g, "row0 ^corner", GroupDomain::Points).unwrap()), 2, "exclude");
        assert_eq!(count(&evaluate(&g, "^corner", GroupDomain::Points).unwrap()), 12, "leading ^ starts from all");
        assert_eq!(count(&evaluate(&g, "!corner", GroupDomain::Points).unwrap()), 12);
        assert_eq!(count(&evaluate(&g, "co*", GroupDomain::Points).unwrap()), 4, "wildcard");
        assert_eq!(count(&evaluate(&g, "*", GroupDomain::Points).unwrap()), 16);
        assert_eq!(count(&evaluate(&g, "0-3 10", GroupDomain::Points).unwrap()), 5);
        assert_eq!(count(&evaluate(&g, "0-15:5", GroupDomain::Points).unwrap()), 4);
        assert_eq!(count(&evaluate(&g, "@P.x>=2", GroupDomain::Points).unwrap()), 8);
        assert_eq!(count(&evaluate(&g, "@ptnum<3", GroupDomain::Points).unwrap()), 3);
        assert_eq!(count(&evaluate(&g, "@primnum<4", GroupDomain::Prims).unwrap()), 4);
        assert!(evaluate(&g, "nope", GroupDomain::Points).unwrap_err().contains("no point group"));
        assert!(evaluate(&g, "@P.y>x", GroupDomain::Points).is_err());
        assert!(evaluate(&g, "@missing>0", GroupDomain::Points).unwrap_err().contains("no attribute"));
    }

    #[test]
    fn create_methods_and_domains() {
        let mesh = grid();
        let random = create(&mesh, "half", GroupDomain::Points, 1, |_| {});
        let n = random.point_attributes["group:half"].iter().filter(|v| **v > 0.5).count();
        assert!((3..=13).contains(&n), "{n}");
        let up = create(&mesh, "up", GroupDomain::Prims, 2, |_| {});
        // Grid triangles face -Y with this winding or +Y; either all or none face up.
        let facing = up.point_attributes["prim:group:up"].iter().filter(|v| **v > 0.5).count();
        assert!(facing == 0 || facing == 18);
        let inverted = create(&mesh, "up", GroupDomain::Prims, 2, |s| s.invert = true);
        assert_eq!(inverted.point_attributes["prim:group:up"].iter().filter(|v| **v > 0.5).count(), 18 - facing);
        // Prim group: guessed from the pattern, and its points.
        let prims = create(&mesh, "first", GroupDomain::Prims, 3, |s| s.expression = "@primnum<2".into());
        assert_eq!(guess_domain(&prims, "first"), GroupDomain::Prims);
        let points = point_mask(&prims, &NodeGroup { pattern: "first".into(), kind: 0 }).unwrap().unwrap();
        assert_eq!(count(&points), 4);
        // Combining with an existing group.
        let both = create(&prims, "first", GroupDomain::Prims, 3, |s| {
            s.expression = "@primnum>=1".into();
            s.combine = 2;
        });
        assert_eq!(both.point_attributes["prim:group:first"].iter().filter(|v| **v > 0.5).count(), 1);
        assert!(group_create(&mesh, &GroupCreateSettings { name: "a b".into(), ..GroupCreateSettings {
            name: String::new(), domain: GroupDomain::Points, method: 0, bbox_min: [0.0; 3], bbox_max: [0.0; 3], percent: 0.0, seed: 0,
            direction: [0.0; 3], spread: 0.0, expression: String::new(), combine: 0, invert: false } }).is_err());
    }

    #[test]
    fn selections_become_compact_patterns() {
        assert_eq!(ranges(&[0, 1, 2, 3, 7, 10, 12, 14, 16, 20]), "0-3 7 10-16:2 20");
        assert_eq!(ranges(&[5]), "5");
        assert_eq!(ranges(&[1, 3]), "1 3");
        assert_eq!(ranges(&[4, 5]), "4-5");
        let mesh = create(&grid(), "corner", GroupDomain::Points, 0, |_| {});
        assert_eq!(describe(&mesh, &(0..16).collect::<Vec<_>>(), GroupDomain::Points), "*");
        assert_eq!(describe(&mesh, &[0, 1, 4, 5], GroupDomain::Points), "corner", "an existing group is named");
        assert_eq!(describe(&mesh, &[], GroupDomain::Points), "");
        // Nearly everything: written as an exclusion.
        let most: Vec<usize> = (0..16).filter(|&i| i != 9).collect();
        assert_eq!(describe(&mesh, &most, GroupDomain::Points), "* ^9");
        // Every description selects exactly what was picked.
        for picked in [vec![2usize, 3, 4, 11], vec![0, 2, 4, 6, 8, 15], most.clone(), vec![1, 5, 0, 4]] {
            let pattern = describe(&mesh, &picked, GroupDomain::Points);
            let back: Vec<usize> = evaluate(&mesh, &pattern, GroupDomain::Points).unwrap().iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect();
            let mut expected = picked.clone();
            expected.sort_unstable();
            assert_eq!(back, expected, "{pattern}");
        }
        assert_eq!(describe(&mesh, &[0, 1, 2], GroupDomain::Prims), "0-2");
    }

    #[test]
    fn restricting_a_change_to_a_group() {
        let mesh = create(&grid(), "left", GroupDomain::Points, 3, |s| s.expression = "@P.x<1".into());
        let mut moved = mesh.clone();
        for p in &mut moved.vertices {
            p.y += 1.0;
        }
        moved.point_attributes.insert("weight".into(), vec![5.0; 16]);
        let group = NodeGroup { pattern: "left".into(), kind: 0 };
        let points = point_mask(&mesh, &group).unwrap().unwrap();
        let prims = prim_mask(&mesh, &group).unwrap().unwrap();
        assert!(restrict_to_group(&mesh, &mut moved, &points, &prims));
        for (i, p) in moved.vertices.iter().enumerate() {
            let inside = mesh.vertices[i].x < 1.0;
            assert_eq!(p.y, if inside { 1.0 } else { 0.0 });
            assert_eq!(moved.point_attributes["weight"][i], if inside { 5.0 } else { 0.0 });
        }
        // Topology changes can't be restricted.
        let mut fewer = mesh.clone();
        fewer.indices.pop();
        assert!(!restrict_to_group(&mesh, &mut fewer, &points, &prims));
        // only_prims keeps per-primitive attributes aligned.
        let mut tagged = mesh.clone();
        tagged.point_attributes.insert("prim:tag".into(), (0..18).map(|i| i as f32).collect());
        let keep: Vec<bool> = (0..18).map(|i| i % 3 == 0).collect();
        let sub = only_prims(&tagged, &keep);
        assert_eq!(sub.indices.len(), 6);
        assert_eq!(sub.point_attributes["prim:tag"], vec![0.0, 3.0, 6.0, 9.0, 12.0, 15.0]);
    }
}
