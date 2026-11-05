// String attributes (s@name in VEX, as in Houdini).
//
// Geometry holds only numbers, so a string attribute is stored as numbers too: each value
// is the id of its text in one table shared by the whole app (id 0 is ""). Kept under the
// attribute's name with "str:" in front ("str:name" on points, "prim:str:name" on
// primitives, "detail:str:name"), it travels through every node that copies, merges, splits
// or filters attributes like any other, and merging stays right because the same text has
// the same id everywhere. Ids only mean something while the app runs: what's saved to disk
// writes the text itself (the Cache node's files), and exports leave string attributes out.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

pub const PREFIX: &str = "str:";

struct Table {
    texts: Vec<Arc<str>>,
    ids: HashMap<Arc<str>, u32>,
}

static TABLE: LazyLock<RwLock<Table>> = LazyLock::new(|| {
    let empty: Arc<str> = "".into();
    RwLock::new(Table { texts: vec![empty.clone()], ids: HashMap::from([(empty, 0)]) })
});

// The id of `text`, added to the table if it's new: exact as an f32 up to 16 million texts.
pub fn id(text: &str) -> f32 {
    if let Some(&id) = TABLE.read().ok().and_then(|t| t.ids.get(text).copied()).as_ref() {
        return id as f32;
    }
    let Ok(mut table) = TABLE.write() else { return 0.0 };
    if let Some(&id) = table.ids.get(text) {
        return id as f32;
    }
    let id = table.texts.len() as u32;
    let text: Arc<str> = text.into();
    table.texts.push(text.clone());
    table.ids.insert(text, id);
    id as f32
}

// The text an id stands for ("" for one that isn't in the table).
pub fn text(id: f32) -> Arc<str> {
    let i = if id.is_finite() && id >= 0.0 { id.round() as usize } else { 0 };
    TABLE.read().ok().and_then(|t| t.texts.get(i).cloned()).unwrap_or_else(|| "".into())
}

// The name to look an attribute up by on `mesh`, given the name users type: "name" finds a
// string attribute (str:name) when there's no number attribute of that name. Strings are
// ids, so nodes that group by an attribute's values (pieces) work with them as they are.
pub fn resolve(mesh: &crate::Mesh, name: &str) -> String {
    let has = |key: String| mesh.point_attributes.contains_key(&key);
    let numbers = ["", "prim:", "vertex:", "detail:"].iter().any(|d| has(format!("{d}{name}")));
    let strings = ["", "prim:", "vertex:", "detail:"].iter().any(|d| has(format!("{d}{PREFIX}{name}")));
    if !numbers && strings { format!("{PREFIX}{name}") } else { name.to_owned() }
}

// Whether an attribute key holds strings ("str:name", "prim:str:name"...).
pub fn is_string_key(key: &str) -> bool {
    let name = ["prim:", "vertex:", "detail:"].iter().find_map(|p| key.strip_prefix(p)).unwrap_or(key);
    name.starts_with(PREFIX)
}

// A string attribute's name as users see it: "str:name" is "name", "prim:str:name" is
// "prim:name".
pub fn display_key(key: &str) -> String {
    for domain in ["prim:", "vertex:", "detail:", ""] {
        if let Some(rest) = key.strip_prefix(domain).and_then(|r| r.strip_prefix(PREFIX)) {
            return format!("{domain}{rest}");
        }
    }
    key.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_has_the_same_id_and_comes_back() {
        let a = id("piece_7");
        assert_eq!(id("piece_7"), a);
        assert_ne!(id("piece_8"), a);
        assert_eq!(&*text(a), "piece_7");
        assert_eq!(id(""), 0.0);
        assert_eq!(&*text(0.0), "");
        assert_eq!(&*text(-3.0), "");
        assert_eq!(&*text(1e9), "", "an id nobody made");
        // Ids survive being carried as numbers, even slightly off.
        assert_eq!(&*text(a + 0.2), "piece_7");
    }

    // Splitting into pieces by a string attribute, typed by its plain name, as in Houdini.
    #[test]
    fn nodes_find_string_attributes_by_their_name() {
        let mut mesh = crate::Mesh::default();
        mesh.vertices = (0..6).map(|i| crate::Vec3::new(i as f32, 0.0, 0.0)).collect();
        mesh.indices = vec![[0, 1, 2], [3, 4, 5], [0, 2, 4]];
        mesh.point_attributes.insert("prim:str:name".into(), vec![id("left"), id("right"), id("left")]);
        assert_eq!(resolve(&mesh, "name"), "str:name");
        let pieces = crate::flow::named_pieces(&mesh, "name").unwrap();
        assert_eq!(pieces.len(), 2, "one piece per name");
        let mut sizes: Vec<usize> = pieces.iter().map(|(_, m)| m.indices.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![1, 2]);
        // A number attribute of that name still wins.
        mesh.point_attributes.insert("prim:name".into(), vec![1.0, 1.0, 1.0]);
        assert_eq!(resolve(&mesh, "name"), "name");
    }

    #[test]
    fn keys_say_whether_they_hold_strings() {
        assert!(is_string_key("str:name") && is_string_key("prim:str:path") && is_string_key("detail:str:label"));
        assert!(!is_string_key("name") && !is_string_key("prim:id") && !is_string_key("group:str"));
        assert_eq!(display_key("prim:str:path"), "prim:path");
        assert_eq!(display_key("str:name"), "name");
        assert_eq!(display_key("Cd"), "Cd");
    }
}
