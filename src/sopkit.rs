// Everyday Houdini SOPs: Extract Centroid, Match Size, Convert Line, Metaball,
// Connectivity and Resample.

use super::{groom, poly, Mesh, Vec3};
use std::collections::HashMap;

// --------------------------------------------------------------------------
// Extract Centroid: one point per piece, at its centre.
// --------------------------------------------------------------------------

pub const PARTITIONS: [&str; 3] = ["Whole geometry", "Each piece (piece or name)", "Connected pieces"];
pub const CENTRES: [&str; 2] = ["Average of points", "Bounding box centre"];

#[derive(Clone, Debug, PartialEq)]
pub struct CentroidSettings {
    pub partition: usize,
    pub method: usize,
}

impl CentroidSettings {
    pub fn defaults() -> Self {
        CentroidSettings { partition: 1, method: 0 }
    }
}

fn pieces_of(input: &Mesh, partition: usize) -> Result<Vec<Mesh>, String> {
    if partition == 0 {
        return Ok(vec![input.clone()]);
    }
    let named = ["piece", "name"].iter().find(|n| input.point_attributes.contains_key(**n) || input.point_attributes.contains_key(&format!("prim:{n}")));
    let settings = match (partition, named) {
        (1, Some(name)) => crate::flow::ForEachSettings { attribute: (*name).to_owned(), ..crate::flow::ForEachSettings::new(crate::flow::NAMED) },
        (1, None) => crate::flow::ForEachSettings::new(crate::flow::CONNECTED),
        _ => crate::flow::ForEachSettings::new(crate::flow::CONNECTED),
    };
    crate::flow::pieces(input, &settings)
}

pub fn extract_centroid(input: &Mesh, set: &CentroidSettings) -> Result<Mesh, String> {
    if input.vertices.is_empty() {
        return Err("the input has no points".into());
    }
    let mut out = Mesh::default();
    let mut piece = Vec::new();
    for (i, part) in pieces_of(input, set.partition)?.into_iter().enumerate() {
        let centre = match set.method {
            1 => part.bounds().map_or(Vec3::zero(), |(lo, hi)| lo.add(hi).scale(0.5)),
            _ => part.vertices.iter().fold(Vec3::zero(), |s, v| s.add(*v)).scale(1.0 / part.vertices.len().max(1) as f32),
        };
        out.vertices.push(centre);
        piece.push(i as f32);
    }
    out.point_attributes.insert("piece".into(), piece);
    Ok(out)
}

// --------------------------------------------------------------------------
// Match Size: moves (and optionally scales) geometry onto a target box.
// --------------------------------------------------------------------------

pub const JUSTIFY: [&str; 4] = ["Min", "Centre", "Max", "Keep"];

#[derive(Clone, Debug, PartialEq)]
pub struct MatchSizeSettings {
    // Per axis: which side lines up with the target (JUSTIFY).
    pub justify: [usize; 3],
    pub scale_to_fit: bool,
    // Scale to fit keeps proportions (the tightest axis decides).
    pub uniform: bool,
    // The target box when there's no second input.
    pub target_center: [f32; 3],
    pub target_size: [f32; 3],
}

impl MatchSizeSettings {
    pub fn defaults() -> Self {
        MatchSizeSettings { justify: [1, 0, 1], scale_to_fit: false, uniform: true, target_center: [0.0; 3], target_size: [1.0; 3] }
    }
}

// `reference`: the second input's geometry, whose bounds are the target.
pub fn match_size(input: &Mesh, reference: Option<&Mesh>, set: &MatchSizeSettings) -> Result<Mesh, String> {
    let (lo, hi) = input.bounds().ok_or("the input has no points")?;
    let (tlo, thi) = match reference {
        Some(r) => r.bounds().ok_or("the reference (input 2) has no points")?,
        // Without scaling, sides line up with the centre point (e.g. Min Y sits on it).
        None if !set.scale_to_fit => (Vec3::from_arr(set.target_center), Vec3::from_arr(set.target_center)),
        None => {
            let (c, s) = (Vec3::from_arr(set.target_center), Vec3::from_arr(set.target_size).scale(0.5));
            (c.sub(s), c.add(s))
        }
    };
    let axis = |v: Vec3, a: usize| [v.x, v.y, v.z][a];
    let size: [f32; 3] = std::array::from_fn(|a| axis(hi, a) - axis(lo, a));
    let target: [f32; 3] = std::array::from_fn(|a| axis(thi, a) - axis(tlo, a));
    let mut scale = [1.0f32; 3];
    if set.scale_to_fit {
        let fit: [f32; 3] = std::array::from_fn(|a| if size[a] > 1e-9 { target[a] / size[a] } else { f32::INFINITY });
        if set.uniform {
            let s = fit.iter().copied().filter(|f| f.is_finite()).fold(f32::INFINITY, f32::min);
            scale = [if s.is_finite() { s } else { 1.0 }; 3];
        } else {
            scale = fit.map(|f| if f.is_finite() { f } else { 1.0 });
        }
    }
    // Where each axis's chosen side is, before (scaled about the input's min) and after.
    let pick = |lo: f32, hi: f32, j: usize| match j {
        0 => lo,
        2 => hi,
        _ => (lo + hi) * 0.5,
    };
    let mut offset = [0.0f32; 3];
    for a in 0..3 {
        let (l, h) = (axis(lo, a), axis(lo, a) + size[a] * scale[a]);
        offset[a] = if set.justify[a] == 3 { 0.0 } else { pick(axis(tlo, a), axis(thi, a), set.justify[a]) - pick(l, h, set.justify[a]) };
    }
    let mut out = input.clone();
    for v in &mut out.vertices {
        let p: [f32; 3] = std::array::from_fn(|a| axis(lo, a) + (axis(*v, a) - axis(lo, a)) * scale[a] + offset[a]);
        *v = Vec3::from_arr(p);
    }
    out.point_attributes.remove("N");
    Ok(out)
}

// --------------------------------------------------------------------------
// Convert Line: every polygon edge as a two-point line.
// --------------------------------------------------------------------------

pub fn convert_line(input: &Mesh) -> Result<Mesh, String> {
    if input.indices.is_empty() {
        return Err("the input has no polygons to turn into lines".into());
    }
    // Each polygon's outline edges once (not the diagonals inside quads).
    let outlines = poly::outline_edges(input);
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for (tri, shown) in input.indices.iter().zip(&outlines) {
        for k in (0..3).filter(|k| shown[*k]) {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            if seen.insert((a.min(b), a.max(b))) {
                edges.push((a, b));
            }
        }
    }
    let n = input.vertices.len();
    let mut out = Mesh::default();
    let mut source = Vec::with_capacity(edges.len() * 2);
    for &(a, b) in &edges {
        out.vertices.extend([input.vertices[a], input.vertices[b]]);
        source.extend([a, b]);
    }
    // Point attributes come along; each line gets its rest length.
    for (key, values) in &input.point_attributes {
        if key.contains(':') || n == 0 || values.len() % n != 0 || key == groom::CURVEU {
            continue;
        }
        let w = values.len() / n;
        out.point_attributes.insert(key.clone(), source.iter().flat_map(|&p| values[p * w..p * w + w].to_vec()).collect());
    }
    out.point_attributes.insert(groom::CURVEU.into(), edges.iter().flat_map(|_| [0.0, 1.0]).collect());
    out.point_attributes.insert("restlength".into(), edges.iter().flat_map(|&(a, b)| { let l = input.vertices[a].sub(input.vertices[b]).length(); [l, l] }).collect());
    Ok(out)
}

// --------------------------------------------------------------------------
// Metaball: smooth blobs around points, joined where they meet.
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct MetaballSettings {
    pub radius: f32,
    // Field value of the surface (0..1): lower is fatter and blends sooner.
    pub threshold: f32,
    // Voxels across the largest dimension.
    pub resolution: usize,
}

impl MetaballSettings {
    pub fn defaults() -> Self {
        MetaballSettings { radius: 1.0, threshold: 0.3, resolution: 48 }
    }
}

// Balls at the input's points (pscale scales the radius), or one at the origin.
pub fn metaball(input: &Mesh, set: &MetaballSettings) -> Result<Mesh, String> {
    let pscale = input.point_attributes.get("pscale").filter(|p| p.len() == input.vertices.len());
    let balls: Vec<(Vec3, f32)> = if input.vertices.is_empty() {
        vec![(Vec3::zero(), set.radius)]
    } else {
        input.vertices.iter().enumerate().map(|(i, p)| (*p, set.radius * pscale.map_or(1.0, |s| s[i]))).filter(|(_, r)| *r > 1e-6).collect()
    };
    if balls.is_empty() {
        return Err("every ball has zero size (pscale 0)".into());
    }
    let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
    for (p, r) in &balls {
        lo = Vec3::new(lo.x.min(p.x - r), lo.y.min(p.y - r), lo.z.min(p.z - r));
        hi = Vec3::new(hi.x.max(p.x + r), hi.y.max(p.y + r), hi.z.max(p.z + r));
    }
    let extent = hi.sub(lo);
    let voxel = extent.x.max(extent.y).max(extent.z) / set.resolution.clamp(8, 256) as f32;
    // One voxel of padding so the surface closes.
    let origin = lo.sub(Vec3::new(voxel, voxel, voxel));
    let dims = [extent.x, extent.y, extent.z].map(|e| (e / voxel).ceil() as usize + 3);
    if dims.iter().product::<usize>() > 16_000_000 {
        return Err("too many voxels: lower the resolution".into());
    }
    let mut values = vec![0.0f32; dims[0] * dims[1] * dims[2]];
    // Wyvill's soft-object falloff: (1 - d²/r²)³ inside each ball, 0 outside.
    for (c, r) in &balls {
        let cell = |v: f32, o: f32| ((v - o) / voxel).floor().max(0.0) as usize;
        let (x0, y0, z0) = (cell(c.x - r, origin.x), cell(c.y - r, origin.y), cell(c.z - r, origin.z));
        let (x1, y1, z1) = (cell(c.x + r, origin.x) + 1, cell(c.y + r, origin.y) + 1, cell(c.z + r, origin.z) + 1);
        for z in z0..=z1.min(dims[2] - 1) {
            for y in y0..=y1.min(dims[1] - 1) {
                for x in x0..=x1.min(dims[0] - 1) {
                    let p = Vec3::new(origin.x + x as f32 * voxel, origin.y + y as f32 * voxel, origin.z + z as f32 * voxel);
                    let q = p.sub(*c).length() / r;
                    if q < 1.0 {
                        values[x + dims[0] * (y + dims[1] * z)] += (1.0 - q * q).powi(3);
                    }
                }
            }
        }
    }
    let field = crate::volume::Volume { class: crate::volume::Class::Fog, voxel, origin, dims, values, background: 0.0 };
    let mut mesh = crate::volume::to_polygons(&field, set.threshold.clamp(0.01, 0.99));
    if mesh.indices.is_empty() {
        return Err("no surface: lower the threshold or grow the radius".into());
    }
    // Outward faces.
    let volume: f32 = mesh.indices.iter().map(|t| { let [a, b, c] = t.map(|i| mesh.vertices[i]); a.dot(b.cross(c)) }).sum();
    if volume < 0.0 {
        mesh.indices.iter_mut().for_each(|t| t.swap(1, 2));
    }
    Ok(mesh)
}

// --------------------------------------------------------------------------
// Connectivity: numbers the connected pieces.
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectivitySettings {
    // 0 points, 1 primitives.
    pub class: usize,
    pub attribute: String,
}

impl ConnectivitySettings {
    pub fn defaults() -> Self {
        ConnectivitySettings { class: 1, attribute: "class".into() }
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

pub fn connectivity(input: &Mesh, set: &ConnectivitySettings) -> Result<Mesh, String> {
    let name = set.attribute.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("'{name}' isn't an attribute name: use letters, digits and _"));
    }
    let n = input.vertices.len();
    let mut parent: Vec<usize> = (0..n).collect();
    for tri in &input.indices {
        let a = find(&mut parent, tri[0]);
        for &v in &tri[1..] {
            let b = find(&mut parent, v);
            parent[b] = a;
        }
    }
    // Pieces numbered 0, 1, 2… in order of their first point.
    let mut number: HashMap<usize, f32> = HashMap::new();
    let point_class: Vec<f32> = (0..n).map(|p| { let root = find(&mut parent, p); let next = number.len() as f32; *number.entry(root).or_insert(next) }).collect();
    let mut out = input.clone();
    if set.class == 0 {
        out.point_attributes.insert(name.to_owned(), point_class);
    } else {
        let prims = input.indices.iter().map(|t| point_class[t[0]]).collect();
        out.point_attributes.insert(format!("prim:{name}"), prims);
    }
    Ok(out)
}

// --------------------------------------------------------------------------
// Resample: evenly spaced points along curves.
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ResampleSettings {
    // On: segments no longer than `length`; off: `segments` per curve.
    pub by_length: bool,
    pub length: f32,
    pub segments: usize,
}

impl ResampleSettings {
    pub fn defaults() -> Self {
        ResampleSettings { by_length: true, length: 0.1, segments: 10 }
    }
}

pub fn resample(input: &Mesh, set: &ResampleSettings) -> Result<Mesh, String> {
    let curves = groom::curves(input);
    if curves.is_empty() {
        return Err("Resample works on curves (for example from Convert Line, or groom curves)".into());
    }
    let n = input.vertices.len();
    let attrs: Vec<(&String, usize)> = input
        .point_attributes
        .iter()
        .filter(|(k, v)| !k.contains(':') && n > 0 && v.len() % n == 0 && k.as_str() != groom::CURVEU)
        .map(|(k, v)| (k, v.len() / n))
        .collect();
    let mut out = Mesh::default();
    let mut values: Vec<Vec<f32>> = attrs.iter().map(|_| Vec::new()).collect();
    let mut curveu = Vec::new();
    for range in curves {
        let pts: Vec<usize> = range.collect();
        // Arc length at each point.
        let mut along = vec![0.0f32];
        for w in pts.windows(2) {
            along.push(along.last().unwrap() + input.vertices[w[1]].sub(input.vertices[w[0]]).length());
        }
        let total = *along.last().unwrap();
        let segments = if set.by_length { (total / set.length.max(1e-4)).ceil().max(1.0) as usize } else { set.segments.max(1) };
        let mut seg = 0;
        for i in 0..=segments {
            let s = total * i as f32 / segments as f32;
            while seg + 2 < along.len() && along[seg + 1] < s {
                seg += 1;
            }
            let span = (along[seg + 1] - along[seg]).max(1e-12);
            let f = ((s - along[seg]) / span).clamp(0.0, 1.0);
            let (a, b) = (pts[seg], pts[seg + 1]);
            out.vertices.push(input.vertices[a].scale(1.0 - f).add(input.vertices[b].scale(f)));
            for ((key, w), store) in attrs.iter().zip(values.iter_mut()) {
                let v = &input.point_attributes[*key];
                store.extend((0..*w).map(|c| v[a * w + c] * (1.0 - f) + v[b * w + c] * f));
            }
            curveu.push(i as f32 / segments as f32);
        }
    }
    for ((key, _), store) in attrs.into_iter().zip(values) {
        out.point_attributes.insert(key.clone(), store);
    }
    out.point_attributes.insert(groom::CURVEU.into(), curveu);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_centroid_and_connectivity() {
        let a = crate::node_box(1.0, 1);
        let b = crate::node_transform(crate::node_box(1.0, 1), [4.0, 0.0, 0.0], [0.0; 3], [1.0; 3]);
        let two = crate::node_merge(a, b);
        let centres = extract_centroid(&two, &CentroidSettings { partition: 2, method: 1 }).unwrap();
        assert_eq!(centres.vertices.len(), 2);
        assert!(centres.vertices.iter().any(|v| v.length() < 1e-5) && centres.vertices.iter().any(|v| (v.x - 4.0).abs() < 1e-5));
        assert_eq!(extract_centroid(&two, &CentroidSettings { partition: 0, method: 1 }).unwrap().vertices, vec![Vec3::new(2.0, 0.0, 0.0)]);
        let classed = connectivity(&two, &ConnectivitySettings::defaults()).unwrap();
        let class = &classed.point_attributes["prim:class"];
        assert_eq!(class.iter().filter(|c| **c == 0.0).count(), 12);
        assert_eq!(class.iter().filter(|c| **c == 1.0).count(), 12);
        let points = connectivity(&two, &ConnectivitySettings { class: 0, attribute: "part".into() }).unwrap();
        assert_eq!(points.point_attributes["part"].iter().copied().fold(0.0f32, f32::max), 1.0);
    }

    #[test]
    fn match_size_moves_and_fits() {
        let cube = crate::node_transform(crate::node_box(2.0, 1), [5.0, 5.0, 5.0], [0.0; 3], [1.0; 3]);
        // Centred in x and z, sitting on y = 0 (the defaults).
        let placed = match_size(&cube, None, &MatchSizeSettings::defaults()).unwrap();
        let (lo, hi) = placed.bounds().unwrap();
        assert!(lo.y.abs() < 1e-5 && (lo.x + hi.x).abs() < 1e-5 && (lo.z + hi.z).abs() < 1e-5);
        // Fitted into a reference's box, keeping proportions.
        let reference = crate::node_grid(1.0, 1.0, 2, 2);
        let set = MatchSizeSettings { scale_to_fit: true, justify: [1, 1, 1], ..MatchSizeSettings::defaults() };
        let fitted = match_size(&cube, Some(&crate::node_transform(crate::node_box(1.0, 1), [0.0; 3], [0.0; 3], [1.0, 4.0, 1.0])), &set).unwrap();
        let (lo, hi) = fitted.bounds().unwrap();
        assert!((hi.x - lo.x - 1.0).abs() < 1e-4 && (hi.y - lo.y - 1.0).abs() < 1e-4, "uniform: the tightest axis decides");
        let _ = reference;
    }

    #[test]
    fn convert_line_and_resample() {
        // A 2×2 quad grid: 12 outline edges, no diagonals.
        let lines = convert_line(&crate::node_grid(2.0, 2.0, 3, 3)).unwrap();
        assert_eq!(lines.vertices.len(), 24);
        assert!(lines.point_attributes["restlength"].iter().all(|l| (l - 1.0).abs() < 1e-5));
        assert_eq!(groom::curves(&lines).len(), 12);
        // Each unit line in segments of at most 0.25: five points.
        let fine = resample(&lines, &ResampleSettings { by_length: true, length: 0.25, segments: 0 }).unwrap();
        assert_eq!(fine.vertices.len(), 12 * 5);
        let three = resample(&lines, &ResampleSettings { by_length: false, length: 0.0, segments: 3 }).unwrap();
        assert_eq!(three.vertices.len(), 12 * 4);
        assert!(resample(&crate::node_box(1.0, 1), &ResampleSettings::defaults()).is_err());
    }

    #[test]
    fn metaballs_blend() {
        let one = metaball(&Mesh::default(), &MetaballSettings::defaults()).unwrap();
        let volume = |m: &Mesh| m.indices.iter().map(|t| { let [a, b, c] = t.map(|i| m.vertices[i]); a.dot(b.cross(c)) / 6.0 }).sum::<f32>();
        // (1 - q²)³ = 0.3 at q ≈ 0.574 of the radius.
        let expected = 4.0 / 3.0 * std::f32::consts::PI * 0.574f32.powi(3);
        assert!((volume(&one) - expected).abs() < 0.1 * expected, "{} vs {expected}", volume(&one));
        // Two close balls make one blob, bigger than the two apart.
        let mut pair = Mesh::default();
        pair.vertices = vec![Vec3::new(-0.4, 0.0, 0.0), Vec3::new(0.4, 0.0, 0.0)];
        let blob = metaball(&pair, &MetaballSettings::defaults()).unwrap();
        assert!(volume(&blob) > 2.0 * expected);
        let parts = connectivity(&blob, &ConnectivitySettings::defaults()).unwrap();
        assert!(parts.point_attributes["prim:class"].iter().all(|c| *c == 0.0), "one piece");
    }
}
