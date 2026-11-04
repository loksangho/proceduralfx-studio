use crate::*;

#[test]
fn edits_are_unsaved_until_saved_and_playback_is_not_an_edit() {
    let mut app = ProceduralFxStudioApp::default();
    app.mark_saved();
    assert!(!app.has_unsaved_changes());
    let grid = app.graph.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
    assert!(app.has_unsaved_changes());
    // Saving (or opening) clears it.
    app.mark_saved();
    assert!(!app.has_unsaved_changes());
    // An animated parameter: playing it back isn't a change.
    app.graph.channels.entry(ChannelId { node: grid, target: ChannelTarget::Param(0) }).or_default().expression = Some("1 + $T".into());
    app.mark_saved();
    app.graph.apply_animation(2.0);
    assert!(!app.has_unsaved_changes());
    app.graph.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    assert!(app.has_unsaved_changes());
}
