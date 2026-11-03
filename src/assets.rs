// Reusable assets, like Houdini's digital assets: a Subnet node packages a network as
// one node, with chosen parameters of the nodes inside "promoted" to it as its own
// sliders. Saved as an asset (a .pfxasset file in the assets folder), it appears in the
// TAB menu, and every copy placed carries the asset's name and version.
//
// A placed asset is still a Subnet with its nodes inside, saved in the scene like any
// other, so scenes open on computers without the asset. "Update" replaces its nodes with
// the library's newer version, keeping the values set on its sliders.
//
// An asset file is a scene file holding just the Subnet and its nodes, with an
// `asset_info` line giving its name, version and description.

use std::path::{Path, PathBuf};

pub type NodeId = usize;

// A parameter of a node inside, shown on the Subnet.
#[derive(Clone, Debug, PartialEq)]
pub struct Promoted {
    pub label: String,
    pub node: NodeId,
    // Which of the node's parameters (its params() index).
    pub index: usize,
    pub min: f32,
    pub max: f32,
}

// What a Subnet is: the asset it was placed from, and its promoted parameters. Stored as
// the node's text, one line each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub asset: Option<(String, u32)>,
    pub description: String,
    pub params: Vec<Promoted>,
}

impl Spec {
    pub fn parse(text: &str) -> Spec {
        let mut spec = Spec::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["asset", name, version] => spec.asset = Some(((*name).to_owned(), version.parse().unwrap_or(1))),
                ["description", text] => spec.description = text.replace("\\n", "\n"),
                ["param", label, node, index, min, max] => {
                    if let (Ok(node), Ok(index)) = (node.parse(), index.parse()) {
                        let number = |s: &str, d: f32| s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(d);
                        spec.params.push(Promoted { label: (*label).to_owned(), node, index, min: number(min, 0.0), max: number(max, 1.0) });
                    }
                }
                _ => {}
            }
        }
        spec
    }

    pub fn format(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n'], " ");
        let mut out = String::new();
        if let Some((name, version)) = &self.asset {
            out += &format!("asset\t{}\t{version}\n", clean(name));
        }
        if !self.description.is_empty() {
            out += &format!("description\t{}\n", self.description.replace('\t', " ").replace('\n', "\\n"));
        }
        for p in &self.params {
            out += &format!("param\t{}\t{}\t{}\t{}\t{}\n", clean(&p.label), p.node, p.index, p.min, p.max);
        }
        out
    }

    // Node ids follow when the nodes are copied with new ids.
    pub fn remap(&mut self, map: impl Fn(NodeId) -> NodeId) {
        for p in &mut self.params {
            p.node = map(p.node);
        }
    }
}

// The asset an .pfxasset file holds: (name, version, description).
pub fn info(text: &str) -> Option<(String, u32, String)> {
    text.lines().find_map(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["asset_info", name, version, description] => Some(((*name).to_owned(), version.parse().ok()?, description.replace("\\n", "\n"))),
            ["asset_info", name, version] => Some(((*name).to_owned(), version.parse().ok()?, String::new())),
            _ => None,
        }
    })
}

pub fn info_line(name: &str, version: u32, description: &str) -> String {
    format!("asset_info\t{}\t{version}\t{}\n", name.replace(['\t', '\n'], " "), description.replace('\t', " ").replace('\n', "\\n"))
}

pub const EXTENSION: &str = "pfxasset";

// An asset in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub path: PathBuf,
}

// The assets in a folder, by name. A name in two files keeps the higher version.
pub fn library(dir: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Entry> = Vec::new();
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == EXTENSION)) {
        let Some((name, version, description)) = std::fs::read_to_string(&path).ok().and_then(|t| info(&t)) else { continue };
        match found.iter_mut().find(|e| e.name == name) {
            Some(existing) if existing.version >= version => {}
            Some(existing) => *existing = Entry { name, version, description, path },
            None => found.push(Entry { name, version, description, path }),
        }
    }
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found
}

// A file name for an asset: its name with anything awkward in file names replaced.
pub fn file_name(name: &str) -> String {
    let stem: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{}.{EXTENSION}", if stem.is_empty() { "asset" } else { &stem })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_through_text() {
        let spec = Spec {
            asset: Some(("Rock\tPile".into(), 3)),
            description: "Rocks\non the ground".into(),
            params: vec![Promoted { label: "Size".into(), node: 4, index: 0, min: 0.0, max: 2.5 }, Promoted { label: "Bumps".into(), node: 7, index: 2, min: -1.0, max: 1.0 }],
        };
        let back = Spec::parse(&spec.format());
        assert_eq!(back.asset, Some(("Rock Pile".into(), 3)), "tabs can't break the line");
        assert_eq!(back.description, "Rocks\non the ground");
        assert_eq!(back.params, spec.params);
        let mut moved = back.clone();
        moved.remap(|id| id + 100);
        assert_eq!(moved.params[1].node, 107);
        assert_eq!(Spec::parse(""), Spec::default());
    }

    #[test]
    fn the_library_lists_each_asset_once_at_its_newest() {
        let dir = std::env::temp_dir().join(format!("pfx_assets_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pfxasset"), format!("pfs 2\n{}", info_line("Rocks", 1, "old"))).unwrap();
        std::fs::write(dir.join("b.pfxasset"), format!("pfs 2\n{}", info_line("Rocks", 2, "new"))).unwrap();
        std::fs::write(dir.join("c.pfxasset"), format!("pfs 2\n{}", info_line("bush", 1, ""))).unwrap();
        std::fs::write(dir.join("d.pfs"), "pfs 2\n").unwrap();
        let found = library(&dir);
        assert_eq!(found.iter().map(|e| (e.name.as_str(), e.version)).collect::<Vec<_>>(), vec![("bush", 1), ("Rocks", 2)]);
        assert_eq!(found[1].description, "new");
        assert_eq!(file_name("Rock pile / v2"), "Rock_pile___v2.pfxasset");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
