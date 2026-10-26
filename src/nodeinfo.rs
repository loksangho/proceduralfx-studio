// Node Info: what a node's geometry holds, at a glance (Houdini's middle-click info).
// Counts, bounds, attributes by class with their sizes, groups, and what kind of
// geometry it is: polygons, curves, points, packed primitives, a volume or a terrain.

use super::Mesh;

// A titled block of (label, value) rows.
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

fn n(count: usize) -> String {
    // 12345678 → 12,345,678
    let s = count.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn v3(v: crate::Vec3) -> String {
    format!("{:.3}, {:.3}, {:.3}", v.x, v.y, v.z)
}

fn bytes(b: usize) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

// Attribute class of a stored key, and its name.
fn class_of(key: &str) -> (&'static str, &str) {
    if let Some(name) = key.strip_prefix("prim:") {
        ("Primitive", name)
    } else if let Some(name) = key.strip_prefix("vertex:") {
        ("Vertex", name)
    } else if let Some(name) = key.strip_prefix("detail:") {
        ("Detail", name)
    } else {
        ("Point", key)
    }
}

pub fn summarize(mesh: &Mesh) -> Vec<Section> {
    let points = mesh.vertices.len();
    let tris = mesh.indices.len();
    let polys = crate::poly::count(mesh);
    let corners: usize = crate::poly::runs(mesh).iter().map(|r| crate::poly::points(mesh, r).len()).sum();
    let curves = crate::groom::curves(mesh).len();
    let mut out = Vec::new();

    // What it is.
    let mut geo = Vec::new();
    let kind = if crate::volume::is_volume(mesh) {
        "Volume"
    } else if mesh.point_attributes.get(crate::terrain::INFO).is_some_and(|i| i.len() >= 4) {
        "Terrain (heightfield)"
    } else if crate::packed::is_packed(mesh) {
        "Packed primitives"
    } else if curves > 0 && tris == 0 {
        "Curves"
    } else if tris == 0 && points > 0 {
        "Points"
    } else if tris > 0 {
        "Polygons"
    } else {
        "Empty"
    };
    geo.push(("Contains".to_string(), kind.to_string()));
    geo.push(("Points".to_string(), n(points)));
    if tris > 0 {
        geo.push(("Primitives".to_string(), format!("{} ({} triangles)", n(polys), n(tris))));
        geo.push(("Vertices".to_string(), n(corners)));
    }
    if curves > 0 {
        geo.push(("Curves".to_string(), n(curves)));
    }
    if crate::packed::is_packed(mesh) {
        geo.push(("Packed".to_string(), format!("{} packed primitives", n(points))));
    }
    if let Ok((voxels, class, voxel, ..)) = crate::volume::decode_voxels(mesh) {
        let class = if class == crate::volume::Class::Fog { "fog (density)" } else { "level set (surface)" };
        geo.push(("Volume".to_string(), format!("{} active voxels, {class}, voxel size {voxel:.4}", n(voxels.len()))));
    }
    if let Some(info) = mesh.point_attributes.get(crate::terrain::INFO).filter(|i| i.len() >= 4) {
        geo.push(("Grid".to_string(), format!("{} × {} points, spacing {:.3} × {:.3}", info[0] as usize, info[1] as usize, info[2], info[3])));
    }
    let memory = points * 12 + tris * 3 * std::mem::size_of::<usize>() + mesh.point_attributes.values().map(|v| v.len() * 4).sum::<usize>();
    geo.push(("Memory".to_string(), format!("about {}", bytes(memory))));
    out.push(Section { title: "Geometry", rows: geo });

    if let Some((lo, hi)) = mesh.bounds() {
        out.push(Section {
            title: "Bounds",
            rows: vec![
                ("Min".to_string(), v3(lo)),
                ("Max".to_string(), v3(hi)),
                ("Size".to_string(), v3(hi.sub(lo))),
                ("Center".to_string(), v3(lo.add(hi).scale(0.5))),
            ],
        });
    }

    // Attributes by class, with their sizes (P is every point's position).
    let mut classes: [(&str, Vec<String>); 4] = [("Point", Vec::new()), ("Primitive", Vec::new()), ("Vertex", Vec::new()), ("Detail", Vec::new())];
    if points > 0 {
        classes[0].1.push("P (3)".into());
    }
    let mut groups: [(&str, Vec<String>); 2] = [("Point", Vec::new()), ("Primitive", Vec::new())];
    let mut keys: Vec<&String> = mesh.point_attributes.keys().collect();
    keys.sort();
    for key in keys {
        if crate::poly::is_internal(key) || key.starts_with("detail:volume_") || key == crate::terrain::INFO || key.starts_with("packed:") {
            continue;
        }
        let values = &mesh.point_attributes[key];
        let (class, name) = class_of(key);
        if let Some(group) = name.strip_prefix("group:") {
            let members = values.iter().filter(|v| **v > 0.5).count();
            let slot = if class == "Primitive" { 1 } else { 0 };
            // A primitive group counts polygons, not triangles.
            let members = if class == "Primitive" && polys > 0 {
                crate::poly::runs(mesh).iter().filter(|r| values.get(r.start).is_some_and(|v| *v > 0.5)).count()
            } else {
                members
            };
            groups[slot].1.push(format!("{group} ({})", n(members)));
            continue;
        }
        let elements = match class {
            "Point" => points,
            "Primitive" => tris,
            "Vertex" => tris * 3,
            _ => 1,
        };
        let size = if class == "Detail" { values.len() } else if elements > 0 { values.len() / elements } else { 0 };
        let slot = ["Point", "Primitive", "Vertex", "Detail"].iter().position(|c| *c == class).unwrap_or(0);
        classes[slot].1.push(format!("{name} ({size})"));
    }
    let rows: Vec<(String, String)> = classes.iter().filter(|(_, l)| !l.is_empty()).map(|(c, l)| (c.to_string(), l.join(", "))).collect();
    if !rows.is_empty() {
        out.push(Section { title: "Attributes", rows });
    }
    let rows: Vec<(String, String)> = groups.iter().filter(|(_, l)| !l.is_empty()).map(|(c, l)| (c.to_string(), l.join(", "))).collect();
    if !rows.is_empty() {
        out.push(Section { title: "Groups", rows });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(sections: &'a [Section], title: &str, label: &str) -> &'a str {
        let s = sections.iter().find(|s| s.title == title).unwrap_or_else(|| panic!("no {title}"));
        &s.rows.iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("no {label} in {title}")).1
    }

    #[test]
    fn summarizes_polygons_attributes_and_groups() {
        let mut grid = crate::node_grid(2.0, 2.0, 3, 3);
        grid.point_attributes.insert("Cd".into(), vec![1.0; 27]);
        grid.point_attributes.insert("group:top".into(), (0..9).map(|i| (i < 3) as u8 as f32).collect());
        grid.point_attributes.insert("detail:iteration".into(), vec![2.0]);
        let info = summarize(&grid);
        assert_eq!(row(&info, "Geometry", "Contains"), "Polygons");
        assert_eq!(row(&info, "Geometry", "Points"), "9");
        assert_eq!(row(&info, "Geometry", "Primitives"), "4 (8 triangles)");
        assert_eq!(row(&info, "Geometry", "Vertices"), "16");
        assert_eq!(row(&info, "Bounds", "Size"), "2.000, 0.000, 2.000");
        let points = row(&info, "Attributes", "Point");
        assert!(points.starts_with("P (3)") && points.contains("Cd (3)"), "{points}");
        assert_eq!(row(&info, "Attributes", "Detail"), "iteration (1)");
        assert_eq!(row(&info, "Groups", "Point"), "top (3)");
        // The polygon bookkeeping attribute isn't shown.
        assert!(!info.iter().flat_map(|s| &s.rows).any(|(_, v)| v.contains("__poly")));
    }

    #[test]
    fn names_what_the_geometry_is() {
        let kind = |m: &Mesh| summarize(m).remove(0).rows.remove(0).1;
        assert_eq!(kind(&Mesh::default()), "Empty");
        let mut cloud = Mesh::default();
        cloud.vertices = vec![crate::Vec3::zero(); 5];
        assert_eq!(kind(&cloud), "Points");
        let fog = crate::volume::encode_voxels(&[([0, 0, 0], 1.0), ([1, 0, 0], 0.5)], crate::volume::Class::Fog, 0.1, crate::Vec3::zero(), 0.0);
        let info = summarize(&fog);
        assert_eq!(row(&info, "Geometry", "Contains"), "Volume");
        assert!(row(&info, "Geometry", "Volume").starts_with("2 active voxels, fog"));
        let t = crate::terrain::terrain(&crate::terrain::TerrainSettings { size: [4.0, 4.0], resolution: 4 });
        assert_eq!(kind(&t), "Terrain (heightfield)");
        assert_eq!(n(12345678), "12,345,678");
    }
}
