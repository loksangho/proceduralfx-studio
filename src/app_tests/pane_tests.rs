use crate::*;

fn tabs(dock: &egui_dock::DockState<Pane>) -> Vec<Pane> {
    let mut t: Vec<Pane> = dock.iter_all_tabs().map(|(_, p)| *p).collect();
    t.sort_by_key(|p| p.title());
    t
}

// A layout that has been drawn (so its areas have real sizes) still saves and loads.
#[test]
fn drawn_layout_saves() {
    struct Blank;
    impl egui_dock::TabViewer for Blank {
        type Tab = Pane;
        fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
            tab.title().into()
        }
        fn ui(&mut self, ui: &mut egui::Ui, _tab: &mut Pane) {
            ui.label("x");
        }
    }
    let ctx = egui::Context::default();
    let mut dock = default_layout();
    for _ in 0..2 {
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() }, |ctx| {
            egui_dock::DockArea::new(&mut dock).show(ctx, &mut Blank);
        });
    }
    let text = ron::to_string(&dock);
    assert!(text.is_ok(), "{text:?}");
    assert!(layout_from_text(&text.unwrap()).is_some());
}

#[test]
fn layout_defaults_saving_and_requests() {
    let dock = default_layout();
    assert_eq!(tabs(&dock), {
        let mut t = vec![Pane::Network, Pane::Parameters, Pane::Timeline, Pane::Viewport];
        t.sort_by_key(|p| p.title());
        t
    });
    let saved = layout_to_text(&dock).unwrap();
    let loaded = layout_from_text(&saved).expect("a saved layout loads back");
    assert_eq!(tabs(&loaded), tabs(&dock));
    assert!(layout_from_text("not a layout").is_none());
    assert_eq!(safe_layout_name("  My Desk/../top "), Some("My Desktop".into()), "path characters are dropped");
    assert_eq!(safe_layout_name("  /// "), None);

    let mut app = ProceduralFxStudioApp::default();
    let mut dock = default_layout();
    app.pane_requests = vec![PaneRequest::Show(Pane::Spreadsheet, true), PaneRequest::Show(Pane::Timeline, false)];
    app.apply_pane_requests(&mut dock);
    let open = tabs(&dock);
    assert!(open.contains(&Pane::Spreadsheet) && !open.contains(&Pane::Timeline));
    // The viewport can't be closed.
    app.pane_requests = vec![PaneRequest::Show(Pane::Viewport, false)];
    app.apply_pane_requests(&mut dock);
    assert!(tabs(&dock).contains(&Pane::Viewport));
    // Popping out moves a pane from the dock into its own window.
    app.pane_requests = vec![PaneRequest::PopOut(Pane::Spreadsheet)];
    app.apply_pane_requests(&mut dock);
    assert!(!tabs(&dock).contains(&Pane::Spreadsheet));
    assert_eq!(app.popped, vec![Pane::Spreadsheet]);
    // Reset brings back the default arrangement.
    app.pane_requests = vec![PaneRequest::Reset];
    app.apply_pane_requests(&mut dock);
    assert_eq!(tabs(&dock), tabs(&default_layout()));
    assert!(app.popped.is_empty());
}
