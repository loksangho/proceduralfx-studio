use crate::*;

#[test]
fn cooking_records_each_nodes_own_time() {
    let _guard = perf::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut g = Graph::default();
    // Ids well clear of the other tests' graphs, which cook at the same time.
    g.next_id = 50_000;
    let grid = g.add(NodeKind::Grid { size: 4.0, res: 200 }, egui::pos2(0.0, 0.0));
    let smooth = g.add(NodeKind::Smooth { iterations: 20, strength: 0.5 }, egui::pos2(0.0, 80.0));
    g.connect(grid, smooth, 0);
    let before = perf::get(smooth).map_or(0, |s| s.cooks);
    g.cook(smooth, 0.0, &mut HashMap::new());
    let (s, gs) = (perf::get(smooth).unwrap(), perf::get(grid).unwrap());
    assert_eq!(s.cooks, before + 1);
    // Smooth's own time leaves out the grid it waited for.
    assert!(s.last_with_inputs >= s.last + gs.last * 0.99, "{s:?} {gs:?}");
    // A cache hit isn't a cook.
    let mut cache = HashMap::new();
    g.cook(smooth, 0.0, &mut cache);
    g.cook(smooth, 0.0, &mut cache);
    assert_eq!(perf::get(smooth).unwrap().cooks, before + 2);
}
