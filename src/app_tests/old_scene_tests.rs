// Scenes made by earlier versions keep opening, and cook as they did.
//
// old_scenes/ holds scenes as earlier versions saved them: every version of each preset
// that shipped (from git history, named by the release it first shipped in), the first
// scene ever committed (in the older "mhip" format), and one of every node type as of a
// date (every_node-<date>.pfs; add a new one when node types are added, and keep the old).
// Never edit these files: they stand for users' saved work.
//
// Each must open, save and open again the same, and cook without errors; what it cooks
// to (points, primitives and bounds, at 0.2 and 0.5 seconds) must match old_scenes.txt within
// a tolerance. When cooking changes on purpose (a solver fix, say), look at what changed,
// then write them again:
//     PFX_WRITE_OLD_SCENES=1 cargo test --release old_scenes
use crate::*;

const RESULTS: &str = include_str!("old_scenes.txt");
// Early (one-shot effects are over by half a second) and later (simulations under way):
// quick to cook either way.
const TIMES: [f32; 2] = [0.2, 0.5];

fn scenes() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app_tests/old_scenes");
    let mut scenes: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap()))
        .collect();
    scenes.sort();
    scenes
}

// What a scene cooks to: for each object shown, its points, primitives and bounds.
fn summary(graph: &Graph) -> Vec<String> {
    let mut objects = graph.geometry_ids();
    objects.sort_unstable();
    TIMES
        .iter()
        .flat_map(|&time| objects.iter().map(move |&id| (time, id)))
        .map(|(time, id)| {
            let mesh = graph.cook_display(&[id], time, &mut HashMap::new());
            let bounds = mesh.bounds().map_or("empty".to_owned(), |(lo, hi)| format!("{:.3} {:.3} {:.3} {:.3} {:.3} {:.3}", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z));
            format!("{time}s/{id}: {} {} {bounds}", mesh.vertices.len(), poly::count(&mesh))
        })
        .collect()
}

// Rigid bodies bouncing off each other amplify the last bits by which platforms' maths
// libraries differ (sin, cos...), so their pieces end up a little apart on each: a Mac
// and Linux differed by 2.3% of the wall's size half a second in. Their bounds get more room.
fn chaotic(scene: &str) -> bool {
    scene.lines().any(|l| l.starts_with("node ") && [" obj rbd_", " obj rbd_bullet", " obj material_fracture"].iter().any(|k| l.contains(k)))
}

// Two summaries of an object agree: counts within 2% (or 2), bounds within 2% of their size
// (6% for rigid bodies, see chaotic).
fn close(saved: &str, now: &str, loose: bool) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (saved.split_whitespace().collect(), now.split_whitespace().collect());
    if a.len() != b.len() || a.first() != b.first() {
        return false;
    }
    let numbers = |v: &[&str]| v.iter().skip(1).map(|t| t.parse::<f64>().ok()).collect::<Option<Vec<f64>>>();
    let (Some(x), Some(y)) = (numbers(&a), numbers(&b)) else { return a == b };
    let counts_ok = (0..2.min(x.len())).all(|i| (x[i] - y[i]).abs() <= (x[i].abs() * 0.02).max(2.0));
    let size = if x.len() == 8 { (0..3).map(|k| x[5 + k] - x[2 + k]).fold(0.0f64, f64::max).max(0.1) } else { 1.0 };
    let room = if loose { 0.06 } else { 0.02 };
    counts_ok && (2..x.len()).all(|i| (x[i] - y[i]).abs() <= size * room + 1e-3)
}

#[test]
fn old_scenes_open_save_and_cook_as_they_did() {
    let saved: HashMap<&str, Vec<&str>> = {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for line in RESULTS.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some((file, rest)) = line.split_once(" | ") {
                map.entry(file).or_default().push(rest);
            }
        }
        map
    };
    let mut written = String::from("# What each scene in old_scenes/ cooks to, at 0.2 and 0.5 seconds, per object shown (see old_scene_tests.rs).\n");
    let mut problems = Vec::new();
    for (file, text) in scenes() {
        let graph = Graph::deserialize(&text);
        if graph.nodes.is_empty() {
            problems.push(format!("{file}: opened with no nodes"));
            continue;
        }
        // Saving and opening again changes nothing.
        let again = graph.serialize();
        if Graph::deserialize(&again).serialize() != again {
            problems.push(format!("{file}: saving and opening again changes it"));
        }
        // Every node type is still there (none turned into something else on loading).
        if file.starts_with("every_node") {
            let count = text.lines().filter(|l| l.starts_with("node ")).count();
            if graph.nodes.len() != count {
                problems.push(format!("{file}: {count} nodes saved, {} opened", graph.nodes.len()));
            }
            continue;
        }
        sims::clear_all();
        let now = summary(&graph);
        for line in &now {
            written += &format!("{file} | {line}\n");
        }
        match saved.get(file.as_str()) {
            None => problems.push(format!("{file}: no saved result (write them again, see the top of old_scene_tests.rs)")),
            Some(before) if before.len() != now.len() || !before.iter().zip(&now).all(|(a, b)| close(a, b, chaotic(&text))) => {
                problems.push(format!("{file}: cooks differently now:\n    was {before:?}\n    now {now:?}"));
            }
            _ => {}
        }
    }
    if std::env::var_os("PFX_WRITE_OLD_SCENES").is_some() {
        std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app_tests/old_scenes.txt"), written).unwrap();
        return;
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn close_strict(saved: &str, now: &str) -> bool {
    close(saved, now, false)
}

#[test]
fn tolerances_allow_noise_but_not_changes() {
    // What Linux and a Mac gave for the Breaking wall: fine for rigid bodies only.
    let (mac, linux) = ("0.5s/0: 602 784 -2.021 -0.019 -2.242 2.051 1.907 0.167", "0.5s/0: 602 784 -2.071 -0.005 -2.337 2.032 1.807 0.162");
    assert!(close(mac, linux, true) && !close(mac, linux, false));
    assert!(chaotic("pfs 2\nnode 5 40 340 0 obj rbd_solver 0 1\n") && !chaotic("pfs 2\nnode 2 40 100 0 obj pyro 56\n"));
    assert!(close_strict("3: 1000 500 0 0 0 1 1 1", "3: 1010 500 0.01 0 0 1 1 1"));
    assert!(!close_strict("0.2s/3: 1000 500 0 0 0 1 1 1", "0.5s/3: 1000 500 0 0 0 1 1 1"), "another time");
    assert!(!close_strict("3: 1000 500 0 0 0 1 1 1", "3: 1100 500 0 0 0 1 1 1"), "10% more points");
    assert!(!close_strict("3: 1000 500 0 0 0 1 1 1", "3: 1000 500 0 0 0 1 1.5 1"), "taller");
    assert!(!close_strict("3: 0 0 empty", "3: 1000 500 0 0 0 1 1 1"));
    assert!(close_strict("3: 0 0 empty", "3: 0 0 empty"));
}
