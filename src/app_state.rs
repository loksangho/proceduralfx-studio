// App: moved out of main.rs.

use crate::*;

#[derive(PartialEq, Clone, Copy)]
pub enum RenderMode {
    Wireframe,
    FlatShaded,
    ShadedWireframe,
    Normals,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub(crate) enum AppContext {
    Obj,
    Mat,
    Out,
    // USD stage (LOP) network.
    Stage,
    // Compositing: COP networks.
    Img,
    // Task graphs: TOP networks.
    Tasks,
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum SelectionMode {
    Vertex,
    Triangle,
    Edge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GizmoHandle {
    Axis(usize),
    Ring(usize),
    // Screen-plane move, or uniform scale.
    Center,
    // A box gizmo's face on `axis`, + or - side: dragging resizes that side.
    Face(usize, bool),
}

// What the gizmo looks like: move/rotate/scale handles, or a box to move and resize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GizmoStyle {
    Xform,
    Box,
}

// The node the gizmo edits. `value` holds translate/rotate/scale (a box: centre in t,
// size in s), in the coordinates of `space`: the enclosing object's transform for
// SOP nodes, identity for objects.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GizmoTarget {
    pub(crate) node: NodeId,
    pub(crate) value: ObjectXform,
    pub(crate) space: ObjectXform,
    pub(crate) style: GizmoStyle,
}

impl GizmoTarget {
    pub(crate) fn point(&self, p: Vec3) -> Vec3 {
        p.mul(Vec3::from_arr(self.space.s)).rotate_deg(self.space.r).add(Vec3::from_arr(self.space.t))
    }

    pub(crate) fn dir(&self, d: Vec3) -> Vec3 {
        d.mul(Vec3::from_arr(self.space.s)).rotate_deg(self.space.r)
    }

    // A world-space offset in the target's own coordinates.
    pub(crate) fn local_dir(&self, d: Vec3) -> Vec3 {
        let r = self.space.r;
        let u = d.rotate_deg([0.0, 0.0, -r[2]]).rotate_deg([0.0, -r[1], 0.0]).rotate_deg([-r[0], 0.0, 0.0]);
        let s = self.space.s.map(|v| if v.abs() < 1e-6 { 1e-6 } else { v });
        Vec3::new(u.x / s[0], u.y / s[1], u.z / s[2])
    }
}

// One handle as drawn last frame, for hit testing and for turning mouse motion into edits.
#[derive(Clone, Debug)]
pub(crate) struct GizmoPart {
    pub(crate) handle: GizmoHandle,
    // Screen polyline (a segment for axes, a loop for rings, one point for the centre).
    pub(crate) points: Vec<egui::Pos2>,
    // Axis handles: screen pixels per world unit along the axis.
    pub(crate) axis_screen: egui::Vec2,
    // Ring handles: whether screen angle increases with the rotation.
    pub(crate) ring_sign: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct GizmoFrame {
    pub(crate) node: NodeId,
    pub(crate) pivot: egui::Pos2,
    pub(crate) pixels_per_unit: f32,
    pub(crate) cam_right: Vec3,
    pub(crate) cam_up: Vec3,
    pub(crate) handle_px: f32,
    pub(crate) parts: Vec<GizmoPart>,
}

impl GizmoFrame {
    pub(crate) fn hit(&self, p: egui::Pos2) -> Option<&GizmoPart> {
        let distance = |part: &GizmoPart| match part.points.as_slice() {
            [c] => c.distance(p),
            points => points
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                    (a + ab * t).distance(p)
                })
                .fold(f32::INFINITY, f32::min),
        };
        self.parts
            .iter()
            .map(|part| (part, distance(part) - if part.handle == GizmoHandle::Center { 4.0 } else { 0.0 }))
            .filter(|(_, d)| *d < 8.0)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(part, _)| part)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GizmoDrag {
    pub(crate) part: GizmoPart,
    pub(crate) frame: GizmoFrame,
    pub(crate) start_pointer: egui::Pos2,
    pub(crate) target: GizmoTarget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SheetDomain {
    Points,
    Vertices,
    Primitives,
    Detail,
}

impl SheetDomain {
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            SheetDomain::Points => "",
            SheetDomain::Vertices => "vertex:",
            SheetDomain::Primitives => "prim:",
            SheetDomain::Detail => "detail:",
        }
    }

    pub(crate) fn count(self, mesh: &Mesh) -> usize {
        match self {
            SheetDomain::Points => mesh.vertices.len(),
            SheetDomain::Vertices => mesh.indices.len() * 3,
            SheetDomain::Primitives => poly::count(mesh),
            SheetDomain::Detail => 1,
        }
    }

    // How many values per component the attribute stores: primitive attributes are per triangle.
    pub(crate) fn stored(self, mesh: &Mesh) -> usize {
        match self {
            SheetDomain::Primitives => mesh.indices.len(),
            other => other.count(mesh),
        }
    }

    // Whether a stored attribute key belongs to this domain.
    pub(crate) fn owns(self, key: &str) -> bool {
        match self {
            SheetDomain::Points => !["prim:", "vertex:", "detail:"].iter().any(|p| key.starts_with(p)),
            _ => key.starts_with(self.prefix()),
        }
    }
}

// An attribute of one domain as the spreadsheet shows it.
pub(crate) struct SheetAttribute {
    pub(crate) name: String,
    pub(crate) key: String,
    pub(crate) components: usize,
    // Values whose length does not match the element count are still shown, but flagged.
    pub(crate) size_ok: bool,
    pub(crate) integral: bool,
}

pub(crate) fn sheet_attributes(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetAttribute> {
    let count = domain.stored(mesh);
    let mut attributes: Vec<SheetAttribute> = mesh
        .point_attributes
        .iter()
        .filter(|(key, _)| domain.owns(key) && !poly::is_internal(key))
        .map(|(key, values)| {
            let components = if count > 0 { (values.len() / count).max(1) } else { 1 };
            SheetAttribute {
                // A string attribute shows by its own name (str:name is name: strings.rs).
                name: key[domain.prefix().len()..].strip_prefix(crate::strings::PREFIX).unwrap_or(&key[domain.prefix().len()..]).to_string(),
                key: key.clone(),
                components,
                size_ok: count > 0 && values.len() == count * components,
                integral: !values.is_empty() && values.iter().take(1024).all(|v| v.fract() == 0.0),
            }
        })
        .collect();
    attributes.sort_by_key(|a| (a.name != "Cd", a.name.to_lowercase()));
    attributes
}

pub(crate) fn component_labels(name: &str, components: usize) -> Vec<String> {
    let suffixes: Vec<String> = match components {
        1 => return vec![name.to_string()],
        3 if matches!(name, "Cd" | "Cs" | "Ce" | "color") => ["r", "g", "b"].map(String::from).to_vec(),
        3 => ["x", "y", "z"].map(String::from).to_vec(),
        4 if name == "orient" => ["x", "y", "z", "w"].map(String::from).to_vec(),
        n => (0..n).map(|c| format!("[{c}]")).collect(),
    };
    suffixes.into_iter().map(|s| if s.starts_with('[') { format!("{name}{s}") } else { format!("{name}.{s}") }).collect()
}

pub(crate) enum SheetSource {
    Index,
    Position(usize),
    ComputedNormal(usize),
    PrimPoints,
    VertexPrim,
    VertexPoint,
    Attribute { key: String, components: usize, component: usize, integral: bool },
}

pub(crate) struct SheetColumn {
    pub(crate) header: String,
    pub(crate) tooltip: String,
    pub(crate) source: SheetSource,
}

// What a standard attribute means, in plain words, for people new to Houdini's names.
pub(crate) fn attribute_meaning(name: &str) -> Option<&'static str> {
    let name = name.rsplit(':').next().unwrap_or(name);
    if let Some(group) = name.strip_prefix("group_") {
        return (!group.is_empty()).then_some("Group membership: 1 if the element is in this group, 0 if not");
    }
    Some(match name {
        "P" => "Position: where the point is (x, y, z)",
        "N" => "Normal: the direction the surface faces",
        "Cd" => "Colour: red, green and blue, from 0 to 1, in linear light (as renderers use it)",
        "Alpha" => "Opacity: 0 is invisible, 1 is solid",
        "v" => "Velocity: how fast and in which direction it moves, in units per second",
        "pscale" => "Point size: the radius particles and copies are drawn at",
        "width" => "Width: how thick a curve or hair is",
        "age" => "Age: how many seconds since the particle was born",
        "life" => "Life: how many seconds the particle lives",
        "id" => "ID: a number that stays with a particle or piece while others are born or die",
        "uv" => "Texture coordinates: where on an image each point samples",
        "orient" => "Orientation: a rotation (quaternion) for copies and pieces",
        "up" => "Up direction: with N, how copies are turned",
        "scale" => "Scale: how much copies are stretched on each axis",
        "rest" => "Rest position: where the point was before it was moved, so noise and textures stick",
        "force" => "Force: a push on the particle, each second",
        "mass" => "Mass: how heavy it is, so how much forces move it",
        "accel" => "Acceleration: how its velocity changes each second",
        "piece" => "Piece: which piece the element belongs to",
        "name" => "Name: text naming the piece (a string attribute)",
        "path" => "Path: text giving the piece's place in a hierarchy (a string attribute)",
        "emission" => "Glow: light it gives off, red, green and blue; above 1 is brighter than white",
        "density" => "Density: how thick smoke or a volume is here",
        "temperature" => "Temperature: how hot the gas is; hotter rises and glows",
        "flame" => "Flame: where the fire burns",
        "dead" => "Dead: 1 when the particle is to be removed",
        "ptnum" => "Point number",
        "primnum" => "Primitive (polygon) number",
        _ => return None,
    })
}

pub(crate) fn sheet_columns(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetColumn> {
    let column = |header: String, tooltip: &str, source| SheetColumn { header, tooltip: tooltip.to_string(), source };
    let mut columns = Vec::new();
    if domain != SheetDomain::Detail {
        columns.push(column("#".into(), "Element number", SheetSource::Index));
    }
    let attributes = sheet_attributes(mesh, domain);
    match domain {
        SheetDomain::Points => {
            for (c, label) in component_labels("P", 3).into_iter().enumerate() {
                columns.push(column(label, "P (3 floats)\nPosition: where the point is (x, y, z)", SheetSource::Position(c)));
            }
        }
        SheetDomain::Vertices => {
            columns.push(column("Prim:Vtx".into(), "Primitive number and the vertex's position in it", SheetSource::VertexPrim));
            columns.push(column("Point".into(), "Point the vertex refers to", SheetSource::VertexPoint));
        }
        SheetDomain::Primitives => {
            columns.push(column("Points".into(), "Point numbers of the polygon, in order", SheetSource::PrimPoints));
        }
        SheetDomain::Detail => {}
    }
    for attribute in attributes {
        let kind = if crate::strings::is_string_key(&attribute.key) { "string" } else if attribute.integral { "ints" } else { "floats" };
        let mut tooltip = format!("{} ({} {kind})", attribute.name, attribute.components);
        if let Some(meaning) = attribute_meaning(&attribute.name) {
            tooltip = format!("{tooltip}\n{meaning}");
        }
        if !attribute.size_ok {
            tooltip.push_str(" · warning: the value count does not match the element count");
        }
        for (component, label) in component_labels(&attribute.name, attribute.components).into_iter().enumerate() {
            columns.push(column(
                label,
                &tooltip,
                SheetSource::Attribute {
                    key: attribute.key.clone(),
                    components: attribute.components,
                    component,
                    integral: attribute.integral,
                },
            ));
        }
    }
    // Normals worked out from the surface, when nothing stores N: after the stored
    // attributes, and only where there's a surface (loose points and particles have none).
    if domain == SheetDomain::Points && !mesh.indices.is_empty() && !mesh.point_attributes.contains_key("N") {
        for (c, label) in component_labels("N", 3).into_iter().enumerate() {
            columns.push(column(label, "N: computed point normal (no N attribute is stored)", SheetSource::ComputedNormal(c)));
        }
    }
    columns
}

// Primitive rows are polygons: `polys` are their triangles, `poly_of` each triangle's polygon.
pub(crate) struct SheetPolys {
    pub(crate) runs: Vec<std::ops::Range<usize>>,
    pub(crate) of: Vec<usize>,
}

impl SheetPolys {
    pub(crate) fn new(mesh: &Mesh) -> Self {
        SheetPolys { runs: poly::runs(mesh), of: poly::of_triangles(mesh) }
    }
}

pub(crate) fn sheet_cell(mesh: &Mesh, normals: &[Vec3], polys: &SheetPolys, source: &SheetSource, row: usize) -> String {
    let float = |v: f32| format!("{v:.3}");
    match source {
        SheetSource::Index => row.to_string(),
        SheetSource::Position(c) => mesh.vertices.get(row).map_or_else(String::new, |p| float([p.x, p.y, p.z][*c])),
        SheetSource::ComputedNormal(c) => normals.get(row).map_or_else(String::new, |n| float([n.x, n.y, n.z][*c])),
        SheetSource::PrimPoints => polys.runs.get(row).map_or_else(String::new, |run| poly::points(mesh, run).iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ")),
        SheetSource::VertexPrim => format!("{}:{}", polys.of.get(row / 3).copied().unwrap_or(row / 3), row % 3),
        SheetSource::VertexPoint => mesh.indices.get(row / 3).map_or_else(String::new, |t| t[row % 3].to_string()),
        SheetSource::Attribute { key, components, component, integral } => {
            // A polygon's primitive attributes are its (first) triangle's.
            let element = if key.starts_with("prim:") { polys.runs.get(row).map_or(row, |run| run.start) } else { row };
            match mesh.point_attributes.get(key).and_then(|values| values.get(element * components + component)) {
                Some(v) if crate::strings::is_string_key(key) => crate::strings::text(*v).to_string(),
                Some(v) if *integral => format!("{}", *v as i64),
                Some(v) => float(*v),
                None => String::new(),
            }
        }
    }
}

// The app's dockable panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum Pane {
    Viewport,
    Network,
    Parameters,
    Timeline,
    Spreadsheet,
    GraphEditor,
    Performance,
}

impl Pane {
    pub(crate) const ALL: [Pane; 7] = [Pane::Viewport, Pane::Network, Pane::Parameters, Pane::Timeline, Pane::Spreadsheet, Pane::GraphEditor, Pane::Performance];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Pane::Viewport => "Viewport",
            Pane::Network => "Network",
            Pane::Parameters => "Parameters",
            Pane::Timeline => "Timeline",
            Pane::Spreadsheet => "Geometry Spreadsheet",
            Pane::GraphEditor => "Graph Editor",
            Pane::Performance => "Performance Monitor",
        }
    }
}

pub(crate) enum PaneRequest {
    Show(Pane, bool),
    PopOut(Pane),
    Reset,
    SaveAs(String),
    Load(String),
    Delete(String),
}

// Network on the left, parameters on the right, timeline under the viewport.
pub(crate) fn default_layout() -> egui_dock::DockState<Pane> {
    use egui_dock::NodeIndex;
    let mut dock = egui_dock::DockState::new(vec![Pane::Viewport]);
    let tree = dock.main_surface_mut();
    let [centre, _network] = tree.split_left(NodeIndex::root(), 0.23, vec![Pane::Network]);
    let [centre, _parameters] = tree.split_right(centre, 0.74, vec![Pane::Parameters]);
    tree.split_below(centre, 0.82, vec![Pane::Timeline]);
    dock
}

// Layouts are RON (it keeps the dock's infinite "not laid out yet" sizes, which JSON can't).
pub(crate) fn layout_file() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layout.ron"))
}

pub(crate) fn layouts_dir() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layouts"))
}

pub(crate) fn layout_to_text(dock: &egui_dock::DockState<Pane>) -> Option<String> {
    ron::to_string(dock).ok()
}

pub(crate) fn layout_from_text(text: &str) -> Option<egui_dock::DockState<Pane>> {
    let dock: egui_dock::DockState<Pane> = ron::from_str(text).ok()?;
    // A layout that lost the viewport isn't worth restoring.
    dock.find_tab(&Pane::Viewport).map(|_| dock)
}

pub(crate) fn load_layout() -> Option<egui_dock::DockState<Pane>> {
    layout_from_text(&std::fs::read_to_string(layout_file()?).ok()?)
}

// Names of the saved layouts, alphabetically.
pub(crate) fn saved_layouts() -> Vec<String> {
    let mut names: Vec<String> = layouts_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| e.path().file_stem().filter(|_| e.path().extension().is_some_and(|x| x == "ron")).map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub(crate) fn safe_layout_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).collect();
    (!name.trim().is_empty()).then(|| name.trim().to_owned())
}

// Draws each tab with the app's own pane functions.
pub(crate) struct PaneViewer<'a> {
    pub(crate) app: &'a mut ProceduralFxStudioApp,
    pub(crate) ctx: egui::Context,
    pub(crate) pop_out: Option<Pane>,
}

impl egui_dock::TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        self.app.pane_ui(ui, &self.ctx, *tab);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Pane, _surface: egui_dock::SurfaceIndex, _node: egui_dock::NodeIndex) {
        if ui.button("Open in New Window").clicked() {
            self.pop_out = Some(*tab);
            ui.close();
        }
    }

    // The viewport stays; everything else can be closed and reopened from Windows.
    fn is_closeable(&self, tab: &Pane) -> bool {
        *tab != Pane::Viewport
    }

    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        match tab {
            Pane::Parameters => [false, true],
            _ => [false, false],
        }
    }
}

// What a viewport pick sets: the node's Group field, Group Create's expression, or a
// Material node slot's group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PickTarget {
    NodeGroup,
    GroupCreate,
    MaterialSlot(usize),
}

pub(crate) struct GroupPick {
    pub(crate) node: NodeId,
    pub(crate) target: PickTarget,
    pub(crate) domain: groups::GroupDomain,
    // The node's input, whose element numbers the group refers to.
    pub(crate) input: Mesh,
    pub(crate) previous_mode: SelectionMode,
}

pub(crate) struct ProceduralFxStudioApp {
    pub(crate) graph: Graph,
    pub(crate) selected: Option<NodeId>,
    // Nodes selected together with `selected` (Shift-click or Shift-drag a box); only
    // counts while it includes `selected`.
    pub(crate) node_selection: Vec<NodeId>,
    // Where a Shift-drag box selection in the network started (screen position).
    pub(crate) node_box: Option<egui::Pos2>,
    // The output being wired: (node, output).
    pub(crate) dragging_link: Option<(NodeId, usize)>,
    pub(crate) graph_pan: egui::Vec2,
    // Where the network editor's canvas was last drawn (end-to-end tests aim at nodes by it).
    pub(crate) network_canvas: egui::Rect,
    pub(crate) graph_zoom: f32,
    pub(crate) menu_pos: egui::Pos2,

    pub(crate) render_mode: RenderMode,
    pub(crate) show_points: bool,
    pub(crate) show_floor: bool,
    pub(crate) show_gizmos: bool,
    pub(crate) gizmo_mode: GizmoMode,
    // Handles drawn last frame, and the handle being dragged.
    pub(crate) gizmo_frame: Option<GizmoFrame>,
    pub(crate) gizmo_drag: Option<GizmoDrag>,
    // A FLIP node's automatic domain (its emitter's bounds), keyed by the upstream hash.
    pub(crate) gizmo_bounds: std::cell::RefCell<Option<(u64, Option<(Vec3, Vec3)>)>>,
    // Puts the cursor in the selected node's Name field (F2 / Rename).
    pub(crate) focus_rename: bool,
    // TOP networks: each node's work item states from the last cook, and the running cook.
    pub(crate) top_results: tops::Results,
    pub(crate) top_run: Option<TopRun>,
    pub(crate) top_cook_request: Option<NodeId>,
    pub(crate) top_cancel_request: bool,
    // The last finished MoonRay render: whether it worked, and the file being written.
    pub(crate) render_ok: Option<bool>,
    pub(crate) pending_render_output: Option<String>,
    // File Output COP: (node, whole timeline range) to write.
    pub(crate) cop_write_request: Option<(NodeId, bool)>,
    // The IMG viewer's texture, keyed by the image buffer it was made from.
    pub(crate) cop_texture: Option<((usize, usize, usize, usize), egui::TextureHandle)>,
    pub(crate) animate: bool,
    // Playback rate in frames per second of real time, and the part-frame owed so far.
    pub(crate) playback_fps: f32,
    pub(crate) playback_due: f32,
    pub(crate) time: f32,
    pub(crate) playback_start: f32,
    pub(crate) playback_end: f32,
    pub(crate) pause_on_edit: bool,
    pub(crate) graph_editor: anim::GraphEditor,
    // Key every parameter that changes (not only already-animated ones).
    pub(crate) auto_key: bool,
    // Selected node's parameters after this frame's animation was applied, for Auto Key.
    pub(crate) anim_snapshot: Option<(NodeId, Vec<f32>, ObjectXform)>,
    // Scene fingerprint last frame, to pause playback on any edit.
    pub(crate) last_content_signature: Option<u64>,

    pub(crate) cam_rot_x: f32,
    pub(crate) cam_rot_y: f32,
    pub(crate) cam_pan_x: f32,
    pub(crate) cam_pan_y: f32,
    pub(crate) cam_zoom: f32,
    pub(crate) cam_target: Vec3,
    pub(crate) camera_focal: f32,
    pub(crate) camera_near: f32,
    pub(crate) orthographic: bool,
    pub(crate) viewport_bg: u8,
    // Flipbook: the capture in progress, the viewer, and where the viewport is in the main window.
    pub(crate) flipbook_recording: Option<flipbook::Recording>,
    // The viewport's surfaces on the GPU, when the app runs on wgpu (else raster.rs).
    pub(crate) gpu_viewport: Option<gpu_viewport::GpuViewport>,
    // The AI assistant panel (bring your own Anthropic API key).
    pub(crate) assistant: assistant::Panel,
    pub(crate) flipbook_viewer: Option<flipbook::Viewer>,
    pub(crate) viewport_rect: Option<egui::Rect>,
    pub(crate) grid_spacing: f32,
    pub(crate) grid_extent: f32,
    pub(crate) ambient_light: f32,
    pub(crate) key_light_dir: [f32; 3],
    pub(crate) key_light_intensity: f32,
    pub(crate) fill_light_dir: [f32; 3],
    pub(crate) fill_light_intensity: f32,
    pub(crate) active_camera: Option<NodeId>,
    // Looking through `active_camera` without moving it: the camera drives the view, and
    // moving the view leaves the camera. Otherwise the view drives the camera.
    pub(crate) looking_through: bool,
    // The view the camera last set, to notice when something else moves the view.
    pub(crate) camera_view: Option<[f32; 11]>,
    pub(crate) look_through_request: Option<Option<NodeId>>,
    pub(crate) use_camera_requested: bool,
    pub(crate) render_camera: RenderCamera,
    // Options of the render being started; set by start_moonray_render.
    pub(crate) render_opts: RenderOpts,
    // Timeline time used when cooking a sequence frame; None renders the current time.
    pub(crate) render_at: Option<f32>,
    pub(crate) sequence: Option<SequenceJob>,
    pub(crate) viewport_focal_px: f32,
    // Geometry node whose network is being edited; None shows the OBJ-level network.
    pub(crate) current_network: Option<NodeId>,
    // Performance monitor: cook times drawn on nodes, the column it sorts by, and the
    // interface's smoothed frame time (seconds).
    pub(crate) show_cook_times: bool,
    // The width the toolbar's render controls took last frame (they move to their own
    // line when the rest of the row leaves less than that).
    pub(crate) toolbar_render_width: f32,
    pub(crate) perf_sort: usize,
    pub(crate) frame_time: f32,

    pub(crate) current_mesh: Mesh,
    // Goes up whenever current_mesh is replaced (the GPU viewport re-uploads it then).
    pub(crate) mesh_revision: u64,
    pub(crate) cook_ms: f32,
    pub(crate) dirty: bool,
    pub(crate) status: String,

    pub(crate) undo: Vec<String>,
    pub(crate) redo: Vec<String>,
    pub(crate) last_state: String,

    pub(crate) template: Option<NodeId>,
    pub(crate) template_mesh: Mesh,
    // Cooked results of nodes that don't change with time, with the fingerprint each was
    // cooked at (Graph::cook_fingerprint): good while it still matches.
    pub(crate) static_cache: HashMap<NodeId, (u64, Mesh)>,
    pub(crate) cache_key: String,
    pub(crate) frame_cache: HashMap<String, Mesh>,
    pub(crate) frame_cache_order: VecDeque<String>,
    pub(crate) frame_cache_hits: usize,
    pub(crate) frame_cache_misses: usize,
    pub(crate) simulation_rx: Option<Receiver<Mesh>>,
    pub(crate) simulation_key: String,
    // The status message a background cook's progress message replaced, put back when the
    // cook finishes (a preset's hint, say).
    pub(crate) status_before_cook: Option<String>,
    pub(crate) cancelled_simulation_key: String,
    pub(crate) simulation_cancel: Option<Arc<AtomicBool>>,
    pub(crate) moonray_rx: Option<Receiver<RenderOutcome>>,
    pub(crate) updater: updater::Updater,
    // The file Save writes to; None until the scene is opened or saved as.
    pub(crate) scene_path: Option<PathBuf>,
    pub(crate) recent_scenes: Vec<PathBuf>,
    pub(crate) window_title: String,
    // Fingerprint of the scene as last opened or saved, for "unsaved changes".
    pub(crate) saved_fingerprint: u64,
    // Unsaved changes, rechecked every half second (for the title and quitting).
    pub(crate) unsaved: bool,
    pub(crate) unsaved_checked: Option<Instant>,
    // Quitting with unsaved changes: the Save / Don't Save / Cancel question is up.
    pub(crate) quit_prompt: bool,
    // The user chose to quit; the next close request goes through.
    pub(crate) quitting: bool,
    // Autosave and crash reports (only in the running app, not tests).
    pub(crate) recovery: Option<recovery::Session>,
    // Unsaved work, or a crash report, left by a session that didn't quit cleanly.
    pub(crate) recovery_offer: Option<recovery::Leftover>,
    // Sending the crash report from the recovery prompt: the note the user adds, and the
    // send in progress (or how it went).
    pub(crate) crash_note: String,
    pub(crate) crash_send: Option<mpsc::Receiver<Result<(), String>>>,
    pub(crate) crash_sent: Option<Result<(), String>>,
    // Where crash reports go: the credits server (tests point it elsewhere).
    pub(crate) crash_report_server: Option<String>,
    // The asset library (the assets folder), as last read.
    pub(crate) asset_library: Vec<assets::Entry>,
    // Where assets are saved: the app's assets folder (tests set their own).
    pub(crate) asset_dir: Option<PathBuf>,
    // The Subnet panel's choices: the node and parameter to promote, its label, and the
    // name and description to save the asset with.
    pub(crate) subnet_promote: (Option<NodeId>, usize, String),
    pub(crate) asset_save: (Option<NodeId>, String, String),
    // Viewport drags paint the selected Attribute Paint node instead of orbiting.
    pub(crate) paint_active: bool,
    pub(crate) paint_fill: Option<NodeId>,
    // Group names on a node's input, for its Group menu: (node, point groups, primitive groups).
    pub(crate) group_choices: Option<(NodeId, Vec<String>, Vec<String>)>,
    pub(crate) group_choices_request: Option<NodeId>,
    // Picking a node's group in the viewport: the viewport shows the node's input meanwhile.
    pub(crate) group_pick: Option<GroupPick>,
    pub(crate) group_pick_request: Option<(NodeId, PickTarget)>,
    // Some(true) accepts the pick, Some(false) cancels it.
    pub(crate) group_pick_finish: Option<bool>,
    pub(crate) group_pick_domain: Option<groups::GroupDomain>,
    pub(crate) moonray_render_requested: bool,
    // Embedded MoonRay defaults and settings
    pub(crate) moonray_default_executable: String,
    pub(crate) moonray_default_arguments: String,
    pub(crate) moonray_default_output: String,
    pub(crate) moonray_default_width: usize,
    pub(crate) moonray_default_height: usize,
    pub(crate) moonray_default_samples: usize,
    pub(crate) show_moonray_settings: bool,
    pub(crate) sprite_sheet: spritesheet::Settings,
    // The Export Vertex Animation window, and a bake running in the background.
    pub(crate) vat_window: VatWindow,
    pub(crate) vat_job: Option<mpsc::Receiver<VatMessage>>,
    // Exporting again as the scene changes (Export Sprite Sheet's "Keep updating").
    pub(crate) live_sheet: Option<LiveSheet>,
    // Render Settings dialog: used by toolbar renders; a selected render node overrides them.
    pub(crate) moonray_default_opts: RenderOpts,
    // Auto-named file in the temp folder instead of moonray_default_output.
    pub(crate) render_to_temp: bool,
    pub(crate) render_temp_ext: &'static str,
    // Scales every render's resolution, render nodes included (quick previews).
    pub(crate) resolution_scale: f32,
    pub(crate) open_viewer_after_render: bool,
    // Render in buckets so the Render Viewer fills in progressively.
    pub(crate) bucket_render: bool,
    pub(crate) render_extra_args: String,
    pub(crate) render_started: Option<Instant>,
    pub(crate) render_cancel: Option<Arc<AtomicBool>>,
    pub(crate) render_log_path: Option<PathBuf>,
    // When the render log was last read for the status bar, and the Windows set-up
    // (installing WSL containers, downloading MoonRay) in progress.
    pub(crate) render_log_checked: Option<Instant>,
    pub(crate) render_setup: Option<mpsc::Receiver<String>>,
    // The set-up window: what the helper said, and whether it's showing.
    pub(crate) render_setup_log: Vec<String>,
    pub(crate) render_setup_open: bool,
    // WSL is missing: the window offers to install it (as an administrator) or to copy
    // the command for one.
    pub(crate) render_setup_needs_admin: bool,
    pub(crate) last_render_log: String,
    pub(crate) last_render_summary: String,
    pub(crate) viewer: RenderViewer,
    pub(crate) node_stats: HashMap<NodeId, (usize, usize)>,
    // The dockable panes' arrangement (taken out while it's drawn), panes popped out into
    // their own OS windows, which panes are open, and pending open/close/pop-out requests.
    pub(crate) dock: Option<egui_dock::DockState<Pane>>,
    pub(crate) popped: Vec<Pane>,
    pub(crate) open_panes: HashSet<Pane>,
    pub(crate) pane_requests: Vec<PaneRequest>,
    pub(crate) saved_layout: String,
    // Some while the "Save Current Layout" name box is open.
    pub(crate) layout_name_prompt: Option<String>,
    // Pyro node whose domain should be fitted around its source this frame.
    pub(crate) pyro_fit_request: Option<NodeId>,
    // The render node's Render button, pressed while its controls drew.
    pub(crate) start_render_request: bool,
    // Why the GPU viewport was given up for the CPU one, if it was (see gpu_fallback).
    pub(crate) gpu_failure: Option<String>,
    // The GPU the app runs on (name, backend, driver), for the log and Copy diagnostics.
    pub(crate) gpu_info: Option<String>,
    // The status line as last logged (applog.rs), so each message is logged once.
    pub(crate) logged_status: String,
    // The preset this scene was started from, for its simple controls (Parameters).
    pub(crate) preset: Option<&'static presets::Preset>,
    // The welcome window (welcome.rs), and its preset pictures once made into textures.
    pub(crate) welcome_open: bool,
    pub(crate) preset_pictures: HashMap<&'static str, egui::TextureHandle>,
    // Make Loop's Find best length, run after the panel draws.
    pub(crate) loop_search_request: Option<NodeId>,
    // USD Export node whose stage should be written this frame.
    pub(crate) usd_export_request: Option<NodeId>,
    // File Export node to write: (node, whole timeline range).
    pub(crate) file_export_request: Option<(NodeId, bool)>,
    // Prim chosen in the scene graph tree.
    pub(crate) stage_selected_prim: Option<String>,
    // Selected LOP's stage, keyed by node and cook generation.
    pub(crate) lop_stage_cache: Option<(NodeId, u64, Arc<usd::Stage>)>,
    pub(crate) sheet_domain: SheetDomain,
    // Show the selected node's own output instead of the displayed geometry.
    pub(crate) sheet_from_selected: bool,
    pub(crate) sheet_selected_only: bool,
    // Selected node's cooked output, keyed by node and cook generation.
    pub(crate) sheet_cache: Option<(NodeId, u64, Mesh)>,
    // Bumped on every graph rebuild so cached cooks know they are stale.
    pub(crate) cook_generation: u64,
    pub(crate) tab_open: bool,
    pub(crate) tab_query: String,
    // The highlighted search result in the add-node menus (arrow keys move it).
    pub(crate) search_pick: usize,
    // The right-click add menu's search text.
    pub(crate) menu_query: String,
    pub(crate) tab_pos: egui::Pos2,
    pub(crate) context: AppContext,
    pub(crate) selected_vertices: Vec<usize>,
    pub(crate) selection_mode: SelectionMode,
    pub(crate) selected_triangles: Vec<usize>,
    pub(crate) selected_edges: Vec<(usize, usize)>,
    pub(crate) isolate_selection: bool,
    pub(crate) selection_box_start: Option<egui::Pos2>,
    pub(crate) selection_box_end: Option<egui::Pos2>,
    // Node Info: the pinned window's node, the node the middle button is held on (and
    // where), and the geometry they show (node, time).
    pub(crate) inspect_node: Option<NodeId>,
    pub(crate) info_hold: Option<(NodeId, egui::Pos2)>,
    pub(crate) info_mesh: Option<(NodeId, f32, Mesh)>,
}

impl Default for ProceduralFxStudioApp {
    fn default() -> Self {
        // An empty scene: new users start from the welcome window (app/welcome.rs), and
        // scenes open from File (not from a scene.pfs that happens to be in the folder).
        let g = Graph::default();
        let (playback_start, playback_end, playback_fps) = (0.0, 20.0, sims::SIM_FPS);
        let selected = None;
        let last_state = g.serialize();
        let saved_fingerprint = scene_fingerprint(&(last_state.clone() + &timeline_line(playback_start, playback_end, playback_fps)));

        let detected_moonray = detect_moonray_executable();
        let default_mr_args = String::from("{scene} {output} {width} {height} {samples}");
        let default_mr_output = home_dir()
            .map(|home| home.join("Pictures").join("render.exr").to_string_lossy().into_owned())
            .unwrap_or_else(|| "render.exr".into());
        let default_mr_w = 1280usize;
        let default_mr_h = 720usize;
        let default_mr_s = 64usize;

        Self {
            graph: g,
            selected,
            node_selection: Vec::new(),
            node_box: None,
            dragging_link: None,
            graph_pan: egui::Vec2::ZERO,
            network_canvas: egui::Rect::NOTHING,
            graph_zoom: 1.0,
            menu_pos: egui::Pos2::ZERO,
            render_mode: RenderMode::ShadedWireframe,
            show_points: false,
            show_floor: true,
            show_gizmos: true,
            gizmo_mode: GizmoMode::Translate,
            gizmo_frame: None,
            gizmo_drag: None,
            gizmo_bounds: std::cell::RefCell::new(None),
            focus_rename: false,
            top_results: HashMap::new(),
            top_run: None,
            top_cook_request: None,
            top_cancel_request: false,
            render_ok: None,
            pending_render_output: None,
            cop_write_request: None,
            cop_texture: None,
            // Not playing until asked (Space, or a preset from the welcome window).
            animate: false,
            playback_fps,
            playback_due: 0.0,
            time: playback_start,
            playback_start,
            playback_end,
            pause_on_edit: true,
            graph_editor: anim::GraphEditor::default(),
            auto_key: false,
            anim_snapshot: None,
            last_content_signature: None,
            cam_rot_x: 0.4,
            cam_rot_y: -0.6,
            cam_pan_x: 0.0,
            cam_pan_y: 0.0,
            cam_zoom: 6.0,
            cam_target: Vec3::zero(),
            camera_focal: 1.2,
            camera_near: 0.5,
            orthographic: false,
            viewport_bg: 45,
            flipbook_recording: None,
            gpu_viewport: None,
            assistant: assistant::Panel::default(),
            flipbook_viewer: None,
            viewport_rect: None,
            grid_spacing: 0.5,
            grid_extent: 5.0,
            ambient_light: 0.25,
            key_light_dir: [0.5, 0.8, -0.6],
            key_light_intensity: 0.75,
            fill_light_dir: [-0.6, 0.2, 0.5],
            fill_light_intensity: 0.25,
            active_camera: None,
            looking_through: false,
            camera_view: None,
            look_through_request: None,
            use_camera_requested: false,
            render_camera: RenderCamera::Auto,
            render_opts: RenderOpts::default(),
            render_at: None,
            sequence: None,
            viewport_focal_px: 720.0 * 1.2,
            current_network: None,
            show_cook_times: false,
            toolbar_render_width: 260.0,
            perf_sort: 0,
            frame_time: 1.0 / 60.0,
            current_mesh: Mesh::default(),
            mesh_revision: 0,
            cook_ms: 0.0,
            dirty: true,
            status: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_state,
            template: None,
            template_mesh: Mesh::default(),
            static_cache: HashMap::new(),
            cache_key: String::new(),
            frame_cache: HashMap::new(),
            frame_cache_order: VecDeque::new(),
            frame_cache_hits: 0,
            frame_cache_misses: 0,
            simulation_rx: None,
            simulation_key: String::new(),
            status_before_cook: None,
            cancelled_simulation_key: String::new(),
            simulation_cancel: None,
            moonray_rx: None,
            updater: updater::Updater::new(),
            scene_path: None,
            recent_scenes: load_recent_scenes(),
            window_title: String::new(),
            saved_fingerprint,
            unsaved: false,
            unsaved_checked: None,
            quit_prompt: false,
            quitting: false,
            recovery: None,
            recovery_offer: None,
            crash_note: String::new(),
            crash_send: None,
            crash_sent: None,
            crash_report_server: assistant::credits_server(),
            asset_library: assets::library(default_asset_dir().as_deref()),
            asset_dir: default_asset_dir(),
            subnet_promote: (None, 0, String::new()),
            asset_save: (None, String::new(), String::new()),
            paint_active: false,
            paint_fill: None,
            group_choices: None,
            group_choices_request: None,
            group_pick: None,
            group_pick_request: None,
            group_pick_finish: None,
            group_pick_domain: None,
            moonray_render_requested: false,
            moonray_default_executable: detected_moonray,
            moonray_default_arguments: default_mr_args,
            moonray_default_output: default_mr_output,
            moonray_default_width: default_mr_w,
            moonray_default_height: default_mr_h,
            moonray_default_samples: default_mr_s,
            show_moonray_settings: false,
            sprite_sheet: spritesheet::Settings::default(),
            vat_window: VatWindow::default(),
            vat_job: None,
            live_sheet: None,
            moonray_default_opts: RenderOpts::default(),
            render_to_temp: true,
            render_temp_ext: "exr",
            resolution_scale: 1.0,
            open_viewer_after_render: true,
            bucket_render: true,
            render_extra_args: String::new(),
            render_started: None,
            render_cancel: None,
            render_log_path: None,
            render_log_checked: None,
            render_setup: None,
            render_setup_log: Vec::new(),
            render_setup_open: false,
            render_setup_needs_admin: false,
            last_render_log: String::new(),
            last_render_summary: String::new(),
            viewer: RenderViewer::default(),
            node_stats: HashMap::new(),
            dock: Some(load_layout().unwrap_or_else(default_layout)),
            popped: Vec::new(),
            open_panes: HashSet::new(),
            pane_requests: Vec::new(),
            saved_layout: String::new(),
            layout_name_prompt: None,
            pyro_fit_request: None,
            start_render_request: false,
            gpu_failure: None,
            gpu_info: None,
            logged_status: String::new(),
            preset: None,
            welcome_open: false,
            preset_pictures: HashMap::new(),
            loop_search_request: None,
            usd_export_request: None,
            file_export_request: None,
            stage_selected_prim: None,
            lop_stage_cache: None,
            sheet_domain: SheetDomain::Points,
            sheet_from_selected: false,
            sheet_selected_only: false,
            sheet_cache: None,
            cook_generation: 0,
            tab_open: false,
            tab_query: String::new(),
            search_pick: 0,
            menu_query: String::new(),
            tab_pos: egui::Pos2::ZERO,
            context: AppContext::Obj,
            selected_vertices: Vec::new(),
            selection_mode: SelectionMode::Vertex,
            selected_triangles: Vec::new(),
            selected_edges: Vec::new(),
            isolate_selection: false,
            selection_box_start: None,
            selection_box_end: None,
            inspect_node: None,
            info_hold: None,
            info_mesh: None,
        }
    }
}

impl ProceduralFxStudioApp {
    // Menu bar for occasional commands, plus one row of the controls used all the time.
    // The menus, also reachable from one ☰ button when the window is too narrow for them.
    pub(crate) const MENUS: [&'static str; 7] = ["File", "Edit", "Select", "View", "Render", "Windows", "Help"];

}
