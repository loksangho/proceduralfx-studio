// ProceduralFX Studio — node-based procedural modeller
// On Windows, a release build is a GUI program (no console window behind it).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Cargo.toml:  eframe = "0.27"   (same egui generation as your original code)

use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc,
};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
#[cfg(feature = "native-openvdb")]
use std::slice;
use std::time::Instant;
use rfd::FileDialog;

mod render_viewer;
mod abc;
mod ai;
mod anim;
mod attrib;
mod assistant;
mod boolean;
mod assets;
mod bullet;
mod cli;
mod color;
mod cops;
mod crowd;
mod deform;
mod docs;
mod envlight;
mod extrude;
mod fields;
mod fileio;
mod flipbook;
mod gpu_viewport;
mod flow;
mod fracture;
mod groom;
mod groups;
mod kinefx;
mod lsystem;
mod looping;
mod measure;
mod motion;
mod normals;
mod muscle;
mod nodeinfo;
mod packed;
mod perf;
mod platform;
mod paint;
mod poly;
mod presets;
mod randomize;
mod recovery;
mod raster;
mod sims;
mod providers;
mod snapshot;
mod shading;
mod sopkit;
mod sops;
mod spritesheet;
mod terrain;
mod testgeo;
mod textures;
mod tops;
mod transfer;
mod enumerate;
mod blur;
mod pop;
mod promote;
mod fluidsurface;
mod updater;
mod vat;
mod vellum;
mod usd;
mod vex;
mod vexsyntax;
mod wslrender;
mod vexmath;
mod volume;
mod vop;
mod whitewater;
mod editor_helpers;
pub(crate) use editor_helpers::*;
mod graph;
pub(crate) use graph::*;
mod mesh;
pub(crate) use mesh::*;
mod node_types;
pub(crate) use node_types::*;
mod sop_ops;
pub(crate) use sop_ops::*;
use render_viewer::{reveal_in_file_browser, RenderViewer};

thread_local! {
    // Previous step's geometry for each SOP Solver being stepped on this thread.
    static SOLVER_PREV_FRAME: std::cell::RefCell<HashMap<NodeId, Mesh>> = std::cell::RefCell::new(HashMap::new());
    // @TimeInc for wrangles: one frame, or one step while a SOP Solver is stepping.
    static SOLVER_TIME_INC: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0 / sims::SIM_FPS) };
    // The geometry each For-Each Begin puts out for the iteration being cooked on this thread.
    static FOREACH_INPUT: std::cell::RefCell<HashMap<NodeId, Mesh>> = std::cell::RefCell::new(HashMap::new());
    // Object Merge nesting, to stop merges that (indirectly) merge themselves.
    static MERGE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FLIP_CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

fn flip_cancelled() -> bool {
    FLIP_CANCEL.with(|flag| {
        flag.borrow()
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    })
}

fn make_png_opaque(path: &std::path::Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "png" {
        return Ok(());
    }

    let mut rgba = image::open(path)
        .map_err(|e| format!("Failed to open PNG {}: {}", path.display(), e))?
        .to_rgba8();
    for px in rgba.pixels_mut() {
        px[3] = 255;
    }
    rgba.save(path)
        .map_err(|e| format!("Failed to write PNG {}: {}", path.display(), e))
}

fn ensure_png_not_all_black(path: &std::path::Path) -> Result<bool, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "png" {
        return Ok(false);
    }

    let mut rgba = image::open(path)
        .map_err(|e| format!("Failed to open PNG {}: {}", path.display(), e))?
        .to_rgba8();

    let is_all_black = rgba.pixels().all(|px| px[0] == 0 && px[1] == 0 && px[2] == 0);
    if !is_all_black {
        return Ok(false);
    }

    let (w, h) = rgba.dimensions();
    let denom_x = (w.saturating_sub(1)).max(1);
    let denom_y = (h.saturating_sub(1)).max(1);
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 32) + (y / 32)) % 2 == 0;
            let base: u8 = if checker { 52 } else { 34 };
            let ramp = (((x as u64) * 80 / (denom_x as u64)) + ((y as u64) * 80 / (denom_y as u64))) as u8;
            let v = base.saturating_add(ramp);
            rgba.put_pixel(x, y, image::Rgba([v, v, v, 255]));
        }
    }

    let cx0 = w / 3;
    let cx1 = (2 * w) / 3;
    let cy0 = h / 3;
    let cy1 = (2 * h) / 3;
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            if (x + y) % 7 == 0 {
                rgba.put_pixel(x, y, image::Rgba([255, 180, 80, 255]));
            }
        }
    }

    rgba.save(path)
        .map_err(|e| format!("Failed to write PNG {}: {}", path.display(), e))?;
    Ok(true)
}

#[cfg(feature = "native-openvdb")]
unsafe extern "C" {
    fn mini_openvdb_surface(
        vertices: *const f32,
        vertex_count: usize,
        triangles: *const u32,
        triangle_count: usize,
        voxel_size: f32,
        padding: u32,
        operation: u32,
        iterations: u32,
        out_vertices: *mut *mut f32,
        out_vertex_count: *mut usize,
        out_triangles: *mut *mut u32,
        out_triangle_count: *mut usize,
    ) -> i32;
    fn mini_openvdb_free(pointer: *mut std::ffi::c_void);
    fn mini_openvdb_volume_from_mesh(
        vertices: *const f32,
        vertex_count: usize,
        triangles: *const u32,
        triangle_count: usize,
        voxel_size: f32,
        padding: u32,
    ) -> *mut std::ffi::c_void;
    fn mini_openvdb_volume_clone(handle: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn mini_openvdb_volume_operation(
        handle: *mut std::ffi::c_void,
        operation: u32,
        iterations: u32,
    ) -> i32;
    fn mini_openvdb_volume_to_mesh(
        handle: *mut std::ffi::c_void,
        out_vertices: *mut *mut f32,
        out_vertex_count: *mut usize,
        out_triangles: *mut *mut u32,
        out_triangle_count: *mut usize,
    ) -> i32;
    fn mini_openvdb_volume_free(handle: *mut std::ffi::c_void);
}

#[cfg(feature = "native-openvdb")]
struct NativeVolume(*mut std::ffi::c_void);

#[cfg(feature = "native-openvdb")]
impl Drop for NativeVolume {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { mini_openvdb_volume_free(self.0) };
        }
    }
}

#[cfg(feature = "native-openvdb")]
impl Clone for NativeVolume {
    fn clone(&self) -> Self {
        Self(unsafe { mini_openvdb_volume_clone(self.0) })
    }
}

// ============================================================================
// 6. APP
// ============================================================================

#[derive(PartialEq, Clone, Copy)]
pub enum RenderMode {
    Wireframe,
    FlatShaded,
    ShadedWireframe,
    Normals,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AppContext {
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
enum SelectionMode {
    Vertex,
    Triangle,
    Edge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoHandle {
    Axis(usize),
    Ring(usize),
    // Screen-plane move, or uniform scale.
    Center,
    // A box gizmo's face on `axis`, + or - side: dragging resizes that side.
    Face(usize, bool),
}

// What the gizmo looks like: move/rotate/scale handles, or a box to move and resize.
#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoStyle {
    Xform,
    Box,
}

// The node the gizmo edits. `value` holds translate/rotate/scale (a box: centre in t,
// size in s), in the coordinates of `space`: the enclosing object's transform for
// SOP nodes, identity for objects.
#[derive(Debug, Clone, Copy)]
struct GizmoTarget {
    node: NodeId,
    value: ObjectXform,
    space: ObjectXform,
    style: GizmoStyle,
}

impl GizmoTarget {
    fn point(&self, p: Vec3) -> Vec3 {
        p.mul(Vec3::from_arr(self.space.s)).rotate_deg(self.space.r).add(Vec3::from_arr(self.space.t))
    }

    fn dir(&self, d: Vec3) -> Vec3 {
        d.mul(Vec3::from_arr(self.space.s)).rotate_deg(self.space.r)
    }

    // A world-space offset in the target's own coordinates.
    fn local_dir(&self, d: Vec3) -> Vec3 {
        let r = self.space.r;
        let u = d.rotate_deg([0.0, 0.0, -r[2]]).rotate_deg([0.0, -r[1], 0.0]).rotate_deg([-r[0], 0.0, 0.0]);
        let s = self.space.s.map(|v| if v.abs() < 1e-6 { 1e-6 } else { v });
        Vec3::new(u.x / s[0], u.y / s[1], u.z / s[2])
    }
}

// One handle as drawn last frame, for hit testing and for turning mouse motion into edits.
#[derive(Clone, Debug)]
struct GizmoPart {
    handle: GizmoHandle,
    // Screen polyline (a segment for axes, a loop for rings, one point for the centre).
    points: Vec<egui::Pos2>,
    // Axis handles: screen pixels per world unit along the axis.
    axis_screen: egui::Vec2,
    // Ring handles: whether screen angle increases with the rotation.
    ring_sign: f32,
}

#[derive(Clone, Debug)]
struct GizmoFrame {
    node: NodeId,
    pivot: egui::Pos2,
    pixels_per_unit: f32,
    cam_right: Vec3,
    cam_up: Vec3,
    handle_px: f32,
    parts: Vec<GizmoPart>,
}

impl GizmoFrame {
    fn hit(&self, p: egui::Pos2) -> Option<&GizmoPart> {
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
struct GizmoDrag {
    part: GizmoPart,
    frame: GizmoFrame,
    start_pointer: egui::Pos2,
    target: GizmoTarget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SheetDomain {
    Points,
    Vertices,
    Primitives,
    Detail,
}

impl SheetDomain {
    fn prefix(self) -> &'static str {
        match self {
            SheetDomain::Points => "",
            SheetDomain::Vertices => "vertex:",
            SheetDomain::Primitives => "prim:",
            SheetDomain::Detail => "detail:",
        }
    }

    fn count(self, mesh: &Mesh) -> usize {
        match self {
            SheetDomain::Points => mesh.vertices.len(),
            SheetDomain::Vertices => mesh.indices.len() * 3,
            SheetDomain::Primitives => poly::count(mesh),
            SheetDomain::Detail => 1,
        }
    }

    // How many values per component the attribute stores: primitive attributes are per triangle.
    fn stored(self, mesh: &Mesh) -> usize {
        match self {
            SheetDomain::Primitives => mesh.indices.len(),
            other => other.count(mesh),
        }
    }

    // Whether a stored attribute key belongs to this domain.
    fn owns(self, key: &str) -> bool {
        match self {
            SheetDomain::Points => !["prim:", "vertex:", "detail:"].iter().any(|p| key.starts_with(p)),
            _ => key.starts_with(self.prefix()),
        }
    }
}

// An attribute of one domain as the spreadsheet shows it.
struct SheetAttribute {
    name: String,
    key: String,
    components: usize,
    // Values whose length does not match the element count are still shown, but flagged.
    size_ok: bool,
    integral: bool,
}

fn sheet_attributes(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetAttribute> {
    let count = domain.stored(mesh);
    let mut attributes: Vec<SheetAttribute> = mesh
        .point_attributes
        .iter()
        .filter(|(key, _)| domain.owns(key) && !poly::is_internal(key))
        .map(|(key, values)| {
            let components = if count > 0 { (values.len() / count).max(1) } else { 1 };
            SheetAttribute {
                name: key[domain.prefix().len()..].to_string(),
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

fn component_labels(name: &str, components: usize) -> Vec<String> {
    let suffixes: Vec<String> = match components {
        1 => return vec![name.to_string()],
        3 if matches!(name, "Cd" | "Cs" | "Ce" | "color") => ["r", "g", "b"].map(String::from).to_vec(),
        3 => ["x", "y", "z"].map(String::from).to_vec(),
        4 if name == "orient" => ["x", "y", "z", "w"].map(String::from).to_vec(),
        n => (0..n).map(|c| format!("[{c}]")).collect(),
    };
    suffixes.into_iter().map(|s| if s.starts_with('[') { format!("{name}{s}") } else { format!("{name}.{s}") }).collect()
}

enum SheetSource {
    Index,
    Position(usize),
    ComputedNormal(usize),
    PrimPoints,
    VertexPrim,
    VertexPoint,
    Attribute { key: String, components: usize, component: usize, integral: bool },
}

struct SheetColumn {
    header: String,
    tooltip: String,
    source: SheetSource,
}

fn sheet_columns(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetColumn> {
    let column = |header: String, tooltip: &str, source| SheetColumn { header, tooltip: tooltip.to_string(), source };
    let mut columns = Vec::new();
    if domain != SheetDomain::Detail {
        columns.push(column("#".into(), "Element number", SheetSource::Index));
    }
    let attributes = sheet_attributes(mesh, domain);
    match domain {
        SheetDomain::Points => {
            for (c, label) in component_labels("P", 3).into_iter().enumerate() {
                columns.push(column(label, "P: point position (3 floats)", SheetSource::Position(c)));
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
        let kind = if attribute.integral { "ints" } else { "floats" };
        let mut tooltip = format!("{} ({} {kind})", attribute.name, attribute.components);
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
struct SheetPolys {
    runs: Vec<std::ops::Range<usize>>,
    of: Vec<usize>,
}

impl SheetPolys {
    fn new(mesh: &Mesh) -> Self {
        SheetPolys { runs: poly::runs(mesh), of: poly::of_triangles(mesh) }
    }
}

fn sheet_cell(mesh: &Mesh, normals: &[Vec3], polys: &SheetPolys, source: &SheetSource, row: usize) -> String {
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
                Some(v) if *integral => format!("{}", *v as i64),
                Some(v) => float(*v),
                None => String::new(),
            }
        }
    }
}

// The app's dockable panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
enum Pane {
    Viewport,
    Network,
    Parameters,
    Timeline,
    Spreadsheet,
    GraphEditor,
    Performance,
}

impl Pane {
    const ALL: [Pane; 7] = [Pane::Viewport, Pane::Network, Pane::Parameters, Pane::Timeline, Pane::Spreadsheet, Pane::GraphEditor, Pane::Performance];

    fn title(self) -> &'static str {
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

enum PaneRequest {
    Show(Pane, bool),
    PopOut(Pane),
    Reset,
    SaveAs(String),
    Load(String),
    Delete(String),
}

// Network on the left, parameters on the right, timeline under the viewport.
fn default_layout() -> egui_dock::DockState<Pane> {
    use egui_dock::NodeIndex;
    let mut dock = egui_dock::DockState::new(vec![Pane::Viewport]);
    let tree = dock.main_surface_mut();
    let [centre, _network] = tree.split_left(NodeIndex::root(), 0.23, vec![Pane::Network]);
    let [centre, _parameters] = tree.split_right(centre, 0.74, vec![Pane::Parameters]);
    tree.split_below(centre, 0.82, vec![Pane::Timeline]);
    dock
}

// Layouts are RON (it keeps the dock's infinite "not laid out yet" sizes, which JSON can't).
fn layout_file() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layout.ron"))
}

fn layouts_dir() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layouts"))
}

fn layout_to_text(dock: &egui_dock::DockState<Pane>) -> Option<String> {
    ron::to_string(dock).ok()
}

fn layout_from_text(text: &str) -> Option<egui_dock::DockState<Pane>> {
    let dock: egui_dock::DockState<Pane> = ron::from_str(text).ok()?;
    // A layout that lost the viewport isn't worth restoring.
    dock.find_tab(&Pane::Viewport).map(|_| dock)
}

fn load_layout() -> Option<egui_dock::DockState<Pane>> {
    layout_from_text(&std::fs::read_to_string(layout_file()?).ok()?)
}

// Names of the saved layouts, alphabetically.
fn saved_layouts() -> Vec<String> {
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

fn safe_layout_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).collect();
    (!name.trim().is_empty()).then(|| name.trim().to_owned())
}

// Draws each tab with the app's own pane functions.
struct PaneViewer<'a> {
    app: &'a mut ProceduralFxStudioApp,
    ctx: egui::Context,
    pop_out: Option<Pane>,
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
enum PickTarget {
    NodeGroup,
    GroupCreate,
    MaterialSlot(usize),
}

struct GroupPick {
    node: NodeId,
    target: PickTarget,
    domain: groups::GroupDomain,
    // The node's input, whose element numbers the group refers to.
    input: Mesh,
    previous_mode: SelectionMode,
}

struct ProceduralFxStudioApp {
    graph: Graph,
    selected: Option<NodeId>,
    // Nodes selected together with `selected` (Shift-click or Shift-drag a box); only
    // counts while it includes `selected`.
    node_selection: Vec<NodeId>,
    // Where a Shift-drag box selection in the network started (screen position).
    node_box: Option<egui::Pos2>,
    // The output being wired: (node, output).
    dragging_link: Option<(NodeId, usize)>,
    graph_pan: egui::Vec2,
    graph_zoom: f32,
    menu_pos: egui::Pos2,

    render_mode: RenderMode,
    show_points: bool,
    show_floor: bool,
    show_gizmos: bool,
    gizmo_mode: GizmoMode,
    // Handles drawn last frame, and the handle being dragged.
    gizmo_frame: Option<GizmoFrame>,
    gizmo_drag: Option<GizmoDrag>,
    // A FLIP node's automatic domain (its emitter's bounds), keyed by the upstream hash.
    gizmo_bounds: std::cell::RefCell<Option<(u64, Option<(Vec3, Vec3)>)>>,
    // Puts the cursor in the selected node's Name field (F2 / Rename).
    focus_rename: bool,
    // TOP networks: each node's work item states from the last cook, and the running cook.
    top_results: tops::Results,
    top_run: Option<TopRun>,
    top_cook_request: Option<NodeId>,
    top_cancel_request: bool,
    // The last finished MoonRay render: whether it worked, and the file being written.
    render_ok: Option<bool>,
    pending_render_output: Option<String>,
    // File Output COP: (node, whole timeline range) to write.
    cop_write_request: Option<(NodeId, bool)>,
    // The IMG viewer's texture, keyed by the image buffer it was made from.
    cop_texture: Option<((usize, usize, usize, usize), egui::TextureHandle)>,
    animate: bool,
    // Playback rate in frames per second of real time, and the part-frame owed so far.
    playback_fps: f32,
    playback_due: f32,
    time: f32,
    playback_start: f32,
    playback_end: f32,
    pause_on_edit: bool,
    graph_editor: anim::GraphEditor,
    // Key every parameter that changes (not only already-animated ones).
    auto_key: bool,
    // Selected node's parameters after this frame's animation was applied, for Auto Key.
    anim_snapshot: Option<(NodeId, Vec<f32>, ObjectXform)>,
    // Scene fingerprint last frame, to pause playback on any edit.
    last_content_signature: Option<u64>,

    cam_rot_x: f32,
    cam_rot_y: f32,
    cam_pan_x: f32,
    cam_pan_y: f32,
    cam_zoom: f32,
    cam_target: Vec3,
    camera_focal: f32,
    camera_near: f32,
    orthographic: bool,
    viewport_bg: u8,
    // Flipbook: the capture in progress, the viewer, and where the viewport is in the main window.
    flipbook_recording: Option<flipbook::Recording>,
    // The viewport's surfaces on the GPU, when the app runs on wgpu (else raster.rs).
    gpu_viewport: Option<gpu_viewport::GpuViewport>,
    // The AI assistant panel (bring your own Anthropic API key).
    assistant: assistant::Panel,
    flipbook_viewer: Option<flipbook::Viewer>,
    viewport_rect: Option<egui::Rect>,
    grid_spacing: f32,
    grid_extent: f32,
    ambient_light: f32,
    key_light_dir: [f32; 3],
    key_light_intensity: f32,
    fill_light_dir: [f32; 3],
    fill_light_intensity: f32,
    active_camera: Option<NodeId>,
    // Looking through `active_camera` without moving it: the camera drives the view, and
    // moving the view leaves the camera. Otherwise the view drives the camera.
    looking_through: bool,
    // The view the camera last set, to notice when something else moves the view.
    camera_view: Option<[f32; 11]>,
    look_through_request: Option<Option<NodeId>>,
    use_camera_requested: bool,
    render_camera: RenderCamera,
    // Options of the render being started; set by start_moonray_render.
    render_opts: RenderOpts,
    // Timeline time used when cooking a sequence frame; None renders the current time.
    render_at: Option<f32>,
    sequence: Option<SequenceJob>,
    viewport_focal_px: f32,
    // Geometry node whose network is being edited; None shows the OBJ-level network.
    current_network: Option<NodeId>,
    // Performance monitor: cook times drawn on nodes, the column it sorts by, and the
    // interface's smoothed frame time (seconds).
    show_cook_times: bool,
    // The width the toolbar's render controls took last frame (they move to their own
    // line when the rest of the row leaves less than that).
    toolbar_render_width: f32,
    perf_sort: usize,
    frame_time: f32,

    current_mesh: Mesh,
    // Goes up whenever current_mesh is replaced (the GPU viewport re-uploads it then).
    mesh_revision: u64,
    cook_ms: f32,
    dirty: bool,
    status: String,

    undo: Vec<String>,
    redo: Vec<String>,
    last_state: String,

    template: Option<NodeId>,
    template_mesh: Mesh,
    // Cooked results of nodes that don't change with time, with the fingerprint each was
    // cooked at (Graph::cook_fingerprint): good while it still matches.
    static_cache: HashMap<NodeId, (u64, Mesh)>,
    cache_key: String,
    frame_cache: HashMap<String, Mesh>,
    frame_cache_order: VecDeque<String>,
    frame_cache_hits: usize,
    frame_cache_misses: usize,
    simulation_rx: Option<Receiver<Mesh>>,
    simulation_key: String,
    cancelled_simulation_key: String,
    simulation_cancel: Option<Arc<AtomicBool>>,
    moonray_rx: Option<Receiver<RenderOutcome>>,
    updater: updater::Updater,
    // The file Save writes to; None until the scene is opened or saved as.
    scene_path: Option<PathBuf>,
    recent_scenes: Vec<PathBuf>,
    window_title: String,
    // Fingerprint of the scene as last opened or saved, for "unsaved changes".
    saved_fingerprint: u64,
    // Unsaved changes, rechecked every half second (for the title and quitting).
    unsaved: bool,
    unsaved_checked: Option<Instant>,
    // Quitting with unsaved changes: the Save / Don't Save / Cancel question is up.
    quit_prompt: bool,
    // The user chose to quit; the next close request goes through.
    quitting: bool,
    // Autosave and crash reports (only in the running app, not tests).
    recovery: Option<recovery::Session>,
    // Unsaved work, or a crash report, left by a session that didn't quit cleanly.
    recovery_offer: Option<recovery::Leftover>,
    // The asset library (the assets folder), as last read.
    asset_library: Vec<assets::Entry>,
    // Where assets are saved: the app's assets folder (tests set their own).
    asset_dir: Option<PathBuf>,
    // The Subnet panel's choices: the node and parameter to promote, its label, and the
    // name and description to save the asset with.
    subnet_promote: (Option<NodeId>, usize, String),
    asset_save: (Option<NodeId>, String, String),
    // Viewport drags paint the selected Attribute Paint node instead of orbiting.
    paint_active: bool,
    paint_fill: Option<NodeId>,
    // Group names on a node's input, for its Group menu: (node, point groups, primitive groups).
    group_choices: Option<(NodeId, Vec<String>, Vec<String>)>,
    group_choices_request: Option<NodeId>,
    // Picking a node's group in the viewport: the viewport shows the node's input meanwhile.
    group_pick: Option<GroupPick>,
    group_pick_request: Option<(NodeId, PickTarget)>,
    // Some(true) accepts the pick, Some(false) cancels it.
    group_pick_finish: Option<bool>,
    group_pick_domain: Option<groups::GroupDomain>,
    moonray_render_requested: bool,
    // Embedded MoonRay defaults and settings
    moonray_default_executable: String,
    moonray_default_arguments: String,
    moonray_default_output: String,
    moonray_default_width: usize,
    moonray_default_height: usize,
    moonray_default_samples: usize,
    show_moonray_settings: bool,
    sprite_sheet: spritesheet::Settings,
    // The Export Vertex Animation window, and a bake running in the background.
    vat_window: VatWindow,
    vat_job: Option<mpsc::Receiver<VatMessage>>,
    // Exporting again as the scene changes (Export Sprite Sheet's "Keep updating").
    live_sheet: Option<LiveSheet>,
    // Render Settings dialog: used by toolbar renders; a selected render node overrides them.
    moonray_default_opts: RenderOpts,
    // Auto-named file in the temp folder instead of moonray_default_output.
    render_to_temp: bool,
    render_temp_ext: &'static str,
    // Scales every render's resolution, render nodes included (quick previews).
    resolution_scale: f32,
    open_viewer_after_render: bool,
    // Render in buckets so the Render Viewer fills in progressively.
    bucket_render: bool,
    render_extra_args: String,
    render_started: Option<Instant>,
    render_cancel: Option<Arc<AtomicBool>>,
    render_log_path: Option<PathBuf>,
    // When the render log was last read for the status bar, and the Windows set-up
    // (installing WSL containers, downloading MoonRay) in progress.
    render_log_checked: Option<Instant>,
    render_setup: Option<mpsc::Receiver<String>>,
    // The set-up window: what the helper said, and whether it's showing.
    render_setup_log: Vec<String>,
    render_setup_open: bool,
    // WSL is missing: the window offers to install it (as an administrator) or to copy
    // the command for one.
    render_setup_needs_admin: bool,
    last_render_log: String,
    last_render_summary: String,
    viewer: RenderViewer,
    node_stats: HashMap<NodeId, (usize, usize)>,
    // The dockable panes' arrangement (taken out while it's drawn), panes popped out into
    // their own OS windows, which panes are open, and pending open/close/pop-out requests.
    dock: Option<egui_dock::DockState<Pane>>,
    popped: Vec<Pane>,
    open_panes: HashSet<Pane>,
    pane_requests: Vec<PaneRequest>,
    saved_layout: String,
    // Some while the "Save Current Layout" name box is open.
    layout_name_prompt: Option<String>,
    // Pyro node whose domain should be fitted around its source this frame.
    pyro_fit_request: Option<NodeId>,
    // Make Loop's Find best length, run after the panel draws.
    loop_search_request: Option<NodeId>,
    // USD Export node whose stage should be written this frame.
    usd_export_request: Option<NodeId>,
    // File Export node to write: (node, whole timeline range).
    file_export_request: Option<(NodeId, bool)>,
    // Prim chosen in the scene graph tree.
    stage_selected_prim: Option<String>,
    // Selected LOP's stage, keyed by node and cook generation.
    lop_stage_cache: Option<(NodeId, u64, Arc<usd::Stage>)>,
    sheet_domain: SheetDomain,
    // Show the selected node's own output instead of the displayed geometry.
    sheet_from_selected: bool,
    sheet_selected_only: bool,
    // Selected node's cooked output, keyed by node and cook generation.
    sheet_cache: Option<(NodeId, u64, Mesh)>,
    // Bumped on every graph rebuild so cached cooks know they are stale.
    cook_generation: u64,
    tab_open: bool,
    tab_query: String,
    // The highlighted search result in the add-node menus (arrow keys move it).
    search_pick: usize,
    // The right-click add menu's search text.
    menu_query: String,
    tab_pos: egui::Pos2,
    context: AppContext,
    selected_vertices: Vec<usize>,
    selection_mode: SelectionMode,
    selected_triangles: Vec<usize>,
    selected_edges: Vec<(usize, usize)>,
    isolate_selection: bool,
    selection_box_start: Option<egui::Pos2>,
    selection_box_end: Option<egui::Pos2>,
    // Node Info: the pinned window's node, the node the middle button is held on (and
    // where), and the geometry they show (node, time).
    inspect_node: Option<NodeId>,
    info_hold: Option<(NodeId, egui::Pos2)>,
    info_mesh: Option<(NodeId, f32, Mesh)>,
}

impl Default for ProceduralFxStudioApp {
    fn default() -> Self {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(40.0, 20.0));
        let grid = g.add(NodeKind::Grid { size: 4.0, res: 20 }, egui::pos2(40.0, 20.0));
        let noise = g.add(NodeKind::Noise { freq: 2.2, amp: 0.65 }, egui::pos2(40.0, 100.0));
        let taper = g.add(NodeKind::Taper { factor: 0.0 }, egui::pos2(40.0, 180.0));
        let twist = g.add(NodeKind::Twist { angle: 1.8 }, egui::pos2(40.0, 260.0));
        for id in [grid, noise, taper, twist] {
            if let Some(node) = g.nodes.get_mut(&id) {
                node.parent = Some(geo);
            }
        }
        g.connect(grid, noise, 0);
        g.connect(noise, taper, 0);
        g.connect(taper, twist, 0);
        g.displays.insert(geo, twist);
        g.add(
            NodeKind::Light {
                position: [3.0, 4.0, 3.0],
                rotation: [-35.0, 35.0, 0.0],
                color: [1.0, 0.95, 0.85],
                intensity: 0.9,
                kind: 0,
            },
            egui::pos2(240.0, 20.0),
        );
        let startup = std::fs::read_to_string(SCENE_FILE).or_else(|_| std::fs::read_to_string(LEGACY_SCENE_FILE)).ok();
        let saved = startup.as_deref().map(Graph::deserialize).filter(|scene| !scene.nodes.is_empty());
        // The saved scene's timeline, if it has one.
        let (playback_start, playback_end, playback_fps) = saved
            .as_ref()
            .and(startup.as_deref())
            .and_then(parse_timeline)
            .unwrap_or((0.0, 20.0, sims::SIM_FPS));
        let g = saved.unwrap_or(g);
        let selected = g.geometry_ids().first().copied();
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
            animate: true,
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
    // The viewport's view settings, to compare with what a camera last set.
    fn view_state(&self) -> [f32; 11] {
        [self.cam_target.x, self.cam_target.y, self.cam_target.z, self.cam_rot_x, self.cam_rot_y, self.camera_focal, self.camera_near, self.orthographic as u8 as f32, self.cam_pan_x, self.cam_pan_y, self.cam_zoom]
    }

    // Puts the view where camera `id` is. False if it isn't a camera.
    fn view_from_camera(&mut self, id: NodeId) -> bool {
        let Some(NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return false;
        };
        self.cam_target = Vec3::from_arr(position);
        self.cam_rot_x = rotation[0].to_radians();
        self.cam_rot_y = rotation[1].to_radians();
        self.camera_focal = focal;
        self.camera_near = near;
        self.orthographic = orthographic;
        (self.cam_pan_x, self.cam_pan_y) = (pan[0], pan[1]);
        self.cam_zoom = zoom;
        true
    }

    // "Use as viewport camera": the selected camera takes the view, and moving the view
    // moves it.
    fn use_requested_camera(&mut self) {
        if !std::mem::take(&mut self.use_camera_requested) {
            return;
        }
        if let Some(id) = self.selected.filter(|id| matches!(self.graph.nodes.get(id).map(|node| &node.kind), Some(NodeKind::Camera { .. }))) {
            if self.view_from_camera(id) {
                self.active_camera = Some(id);
                self.looking_through = false;
                self.camera_view = None;
                self.status = format!("Viewport camera {id} activated");
            }
        }
    }

    // Looks through camera `id` without changing it (None: back to the free view, which
    // stays where it is).
    fn look_through(&mut self, id: Option<NodeId>) {
        match id {
            Some(id) if self.view_from_camera(id) => {
                self.active_camera = Some(id);
                self.looking_through = true;
                self.camera_view = Some(self.view_state());
                let name = self.graph.nodes.get(&id).map_or(String::new(), |n| n.title(id));
                self.status = format!("Looking through {name}; moving the view leaves it unchanged");
            }
            _ => {
                if self.looking_through {
                    self.active_camera = None;
                }
                self.looking_through = false;
                self.camera_view = None;
            }
        }
        self.dirty = true;
    }

    fn sync_active_camera_from_viewport(&mut self) {
        if self.looking_through {
            return;
        }
        let Some(id) = self.active_camera else {
            return;
        };
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            self.active_camera = None;
            return;
        };
        let NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } = &mut node.kind else {
            self.active_camera = None;
            return;
        };
        *position = [self.cam_target.x, self.cam_target.y, self.cam_target.z];
        *rotation = [
            self.cam_rot_x.to_degrees(),
            self.cam_rot_y.to_degrees(),
            rotation[2],
        ];
        *focal = self.camera_focal;
        *near = self.camera_near;
        *orthographic = self.orthographic;
        *pan = [self.cam_pan_x, self.cam_pan_y];
        *zoom = self.cam_zoom;
    }

    // The scene's cameras and their names, as the camera menus list them.
    fn camera_ids(&self) -> Vec<(NodeId, String)> {
        let mut cameras: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. }))
            .map(|(id, n)| (*id, n.title(*id)))
            .collect();
        cameras.sort_unstable();
        cameras
    }

    fn resolve_render_camera(&self) -> (CamRig, String) {
        let node_rig = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Camera { position, rotation, focal, orthographic, pan, zoom, .. }) => Some((
                CamRig {
                    target: Vec3::from_arr(*position),
                    rot_x: rotation[0].to_radians(),
                    rot_y: rotation[1].to_radians(),
                    zoom: *zoom,
                    focal: *focal,
                    orthographic: *orthographic,
                    pan: *pan,
                },
                self.graph.nodes[&id].title(id),
            )),
            _ => None,
        };
        let chosen = match self.render_opts.camera {
            RenderCamera::Node(id) => node_rig(id),
            RenderCamera::Auto => self.selected.and_then(node_rig),
            RenderCamera::Viewport => None,
        };
        chosen.unwrap_or_else(|| {
            (
                CamRig {
                    target: self.cam_target,
                    rot_x: self.cam_rot_x,
                    rot_y: self.cam_rot_y,
                    zoom: self.cam_zoom,
                    focal: self.camera_focal,
                    orthographic: self.orthographic,
                    pan: [self.cam_pan_x, self.cam_pan_y],
                },
                "viewport".to_string(),
            )
        })
    }

    fn start_moonray_render(&mut self) {
        if self.moonray_rx.is_some() || self.sequence.is_some() {
            self.status = "A MoonRay render is already running".into();
            return;
        }
        self.status.clear();
        self.render_at = None;
        // A selected render node supplies its own settings; otherwise the toolbar defaults apply.
        if let Some(id) = self.selected {
            if let Some(NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts }) =
                self.graph.nodes.get(&id).map(|node| &node.kind)
            {
                self.render_opts = opts.clone();
                if opts.sequence {
                    let job = SequenceJob {
                        executable: executable.clone(),
                        arguments: arguments.clone(),
                        output: output.clone(),
                        width: *width,
                        height: *height,
                        samples: *samples,
                        opts: opts.clone(),
                        times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                        next: 0,
                    };
                    self.begin_sequence(job);
                    return;
                }
                self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
                self.spawn_moonray_render(
                    executable.clone(),
                    arguments.clone(),
                    output.clone(),
                    *width,
                    *height,
                    *samples,
                );
                return;
            }
        }
        // Otherwise the Render Settings dialog applies.
        let opts = RenderOpts { camera: self.render_camera, ..self.moonray_default_opts.clone() };
        self.render_opts = opts.clone();
        // Relative outputs are routed to the temp folder by spawn_moonray_render.
        let output = if self.render_to_temp {
            format!("render.{}", self.render_temp_ext)
        } else {
            self.moonray_default_output.clone()
        };
        if opts.sequence {
            let job = SequenceJob {
                executable: self.moonray_default_executable.clone(),
                arguments: self.moonray_default_arguments.clone(),
                output,
                width: self.moonray_default_width,
                height: self.moonray_default_height,
                samples: self.moonray_default_samples,
                times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                opts,
                next: 0,
            };
            self.begin_sequence(job);
            return;
        }
        self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
        self.spawn_moonray_render(
            self.moonray_default_executable.clone(),
            self.moonray_default_arguments.clone(),
            output,
            self.moonray_default_width,
            self.moonray_default_height,
            self.moonray_default_samples,
        );
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pane: Pane) {
        match pane {
            Pane::Viewport => self.viewport(ui, ctx),
            Pane::Network => self.node_editor(ui),
            Pane::Parameters => self.parameters_pane(ui),
            Pane::Timeline => self.timeline_pane(ui),
            Pane::Spreadsheet => self.spreadsheet_pane(ui),
            Pane::GraphEditor => self.graph_editor_pane(ui),
            Pane::Performance => self.performance_pane(ui),
        }
    }

    fn apply_pane_requests(&mut self, dock: &mut egui_dock::DockState<Pane>) {
        for request in std::mem::take(&mut self.pane_requests) {
            match request {
                PaneRequest::Reset => {
                    *dock = default_layout();
                    self.popped.clear();
                }
                PaneRequest::SaveAs(name) => {
                    let saved = safe_layout_name(&name).zip(layouts_dir()).zip(layout_to_text(dock)).map(|((name, dir), text)| {
                        let _ = std::fs::create_dir_all(&dir);
                        (std::fs::write(dir.join(format!("{name}.ron")), text), name)
                    });
                    self.status = match saved {
                        Some((Ok(()), name)) => format!("Layout \"{name}\" saved"),
                        Some((Err(e), _)) => format!("Couldn't save the layout: {e}"),
                        None => "Give the layout a name (letters, numbers, spaces, - or _)".into(),
                    };
                }
                PaneRequest::Load(name) => {
                    match layouts_dir().and_then(|dir| std::fs::read_to_string(dir.join(format!("{name}.ron"))).ok()).and_then(|t| layout_from_text(&t)) {
                        Some(loaded) => {
                            *dock = loaded;
                            self.popped.clear();
                            self.status = format!("Layout \"{name}\"");
                        }
                        None => self.status = format!("Couldn't read layout \"{name}\""),
                    }
                }
                PaneRequest::Delete(name) => {
                    if let Some(dir) = layouts_dir() {
                        let _ = std::fs::remove_file(dir.join(format!("{name}.ron")));
                    }
                    self.status = format!("Layout \"{name}\" deleted");
                }
                PaneRequest::Show(pane, true) => {
                    if dock.find_tab(&pane).is_none() && !self.popped.contains(&pane) {
                        // A new pane goes next to the viewport.
                        let at = dock.find_tab(&Pane::Viewport).map(|(surface, node, _)| (surface, node));
                        match at {
                            Some((surface, node)) if surface == egui_dock::SurfaceIndex::main() => {
                                dock.main_surface_mut().split_below(node, 0.62, vec![pane]);
                            }
                            _ => dock.push_to_first_leaf(pane),
                        }
                    }
                }
                PaneRequest::Show(pane, false) => {
                    if pane != Pane::Viewport {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.retain(|p| *p != pane);
                    }
                }
                PaneRequest::PopOut(pane) => {
                    if !self.popped.contains(&pane) {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.push(pane);
                    }
                }
            }
        }
    }

    fn show_panes(&mut self, ctx: &egui::Context) {
        if let Some(mut name) = self.layout_name_prompt.take() {
            let (mut keep, mut open) = (true, true);
            own_window(ctx, "save_layout", "Save Layout", [340.0, 130.0], [300.0, 110.0], &mut open, true, |ui| {
                ui.label("Layout name");
                let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("e.g. Modelling, Rendering"));
                field.request_focus();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || enter {
                        self.pane_requests.push(PaneRequest::SaveAs(name.clone()));
                        keep = false;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        keep = false;
                    }
                });
                if saved_layouts().iter().any(|n| Some(n) == safe_layout_name(&name).as_ref()) {
                    ui.weak("Saving replaces the layout with this name.");
                }
            });
            // Closing its window cancels it too.
            if keep && open {
                self.layout_name_prompt = Some(name);
            }
        }
        let mut dock = self.dock.take().unwrap_or_else(default_layout);
        self.apply_pane_requests(&mut dock);
        let pop_out = {
            let mut viewer = PaneViewer { app: self, ctx: ctx.clone(), pop_out: None };
            egui_dock::DockArea::new(&mut dock)
                .style(egui_dock::Style::from_egui(ctx.style().as_ref()))
                .show_leaf_collapse_buttons(false)
                .show(ctx, &mut viewer);
            viewer.pop_out
        };
        if let Some(pane) = pop_out {
            self.pane_requests.push(PaneRequest::PopOut(pane));
        }
        // Popped-out panes: real OS windows; closing one puts the pane back in the dock.
        let mut redock = Vec::new();
        for pane in self.popped.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} — ProceduralFX Studio", pane.title()))
                .with_inner_size(match pane {
                    Pane::Viewport => [900.0, 640.0],
                    Pane::Timeline => [900.0, 160.0],
                    _ => [560.0, 520.0],
                });
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("pane", pane.title())), builder, |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| self.pane_ui(ui, ctx, pane));
                if ctx.input(|i| i.viewport().close_requested()) {
                    redock.push(pane);
                }
            });
        }
        for pane in redock {
            self.popped.retain(|p| *p != pane);
            self.pane_requests.push(PaneRequest::Show(pane, true));
        }
        self.open_panes = dock.iter_all_tabs().map(|(_, tab)| *tab).chain(self.popped.iter().copied()).collect();
        // Keep the arrangement for next time.
        if let Some(text) = layout_to_text(&dock) {
            if text != self.saved_layout {
                if let Some(file) = layout_file() {
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(file, &text);
                }
                self.saved_layout = text;
            }
        }
        self.dock = Some(dock);
    }

    fn parameters_pane(&mut self, ui: &mut egui::Ui) {
        let before = self.selected.and_then(|id| self.graph.nodes.get(&id).map(|n| (id, n.kind.params(), n.xform)));
        TYPED.with(|t| *t.borrow_mut() = None);
        EXPRESSION_TIME.with(|t| t.set(self.time));
        let edited = self.params_ui(ui);
        if let (Some(typed), Some(before)) = (TYPED.with(|t| t.borrow_mut().take()), before) {
            self.apply_typed(before, typed);
        }
        if edited {
            if self.pause_on_edit {
                self.animate = false;
            }
            self.dirty = true;
        }
        // The shortcut notes sit at the bottom of the pane when there's room, and after
        // the parameters when there isn't (so they never cover them).
        let notes = egui::RichText::new(
            "Network: RMB add node • drag output→input to wire\n\
                     click input to disconnect • dbl-click = display\n\
                     OBJ: Geometry / Light / Camera nodes need no wiring •\n\
                     dbl-click Geometry to enter • U = up • dbl-click Light/Camera = activate\n\
                     MMB hold on a node = Node Info\n\
                     TAB search/add • H home network • T template\n\
                     B bypass • Ctrl+D duplicate • Del delete\n\
                     Ctrl+Z / Ctrl+Shift+Z undo/redo • Space play\n\
                     Selection: All • None • Invert • Grow • Shrink • Boundary • Non-manifold • Isolated • Connected\n\
                     Viewport: LMB orbit • RMB pan • wheel zoom • F frame",
        )
        .small()
        .weak();
        let galley = egui::WidgetText::from(notes.clone()).into_galley(ui, Some(egui::TextWrapMode::Wrap), ui.available_width(), egui::TextStyle::Small);
        let spare = ui.available_height() - galley.size().y - ui.spacing().item_spacing.y * 2.0;
        if spare > 0.0 && spare.is_finite() {
            ui.add_space(spare);
        } else {
            ui.add_space(12.0);
        }
        ui.label(notes);
    }

    fn timeline_pane(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮ Start").on_hover_text("Jump to the playback start").clicked() {
                    self.time = self.playback_start;
                    self.dirty = true;
                }
                if ui.add(egui::Button::new(if self.animate { "⏸ Pause" } else { "▶ Play" })).on_hover_text("Space toggles playback").clicked() {
                    self.animate = !self.animate;
                    self.dirty = true;
                }
                if ui.button("⏭ End").on_hover_text("Jump to the playback end").clicked() {
                    self.time = self.playback_end;
                    self.dirty = true;
                }
                let flipbook = ui
                    .add_enabled(self.flipbook_recording.is_none(), egui::Button::new("🎞 Flipbook"))
                    .on_hover_text("Plays the playback range in the viewport, captures every frame, and plays them back (save as PNG frames or a movie)");
                if flipbook.clicked() {
                    self.start_flipbook();
                }
                ui.label(format!("Time: {:.2}", self.time));
                pfield(ui, pdrag(&mut self.playback_fps).range(1.0..=120.0).speed(0.5).fixed_decimals(0).suffix(" fps"))
                    .on_hover_text(format!("Playback rate. Scenes are {} frames a second, so {} plays in real time; frames that take longer to cook are waited for, never skipped", sims::SIM_FPS, sims::SIM_FPS));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.status.is_empty() {
                        ui.label(egui::RichText::new(&self.status).color(egui::Color32::LIGHT_GREEN));
                    }
                });
            });

            // Scrub bar: dragging pauses playback and the viewport follows the time.
            ui.horizontal(|ui| {
                let frame = (self.time * sims::SIM_FPS).round() as i64 + 1;
                ui.monospace(format!("Frame {frame:>4}"));
                ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                let scrub = ui
                    .add(pslider(&mut self.time, self.playback_start..=self.playback_end).suffix(" s").show_value(true))
                    .on_hover_text("Drag to scrub the timeline (← / → step one frame)");
                if scrub.drag_started() {
                    self.animate = false;
                }
                // Ticks at the selected node's keys.
                if let Some(id) = self.selected {
                    let span = (self.playback_end - self.playback_start).max(1e-6);
                    let rail = scrub.rect.height() * 0.5;
                    let width = ui.spacing().slider_width - rail * 2.0;
                    let times: Vec<f32> = self.graph.node_channels(id).flat_map(|(_, ch)| ch.keys.iter().map(|k| k.t)).collect();
                    for t in times.into_iter().filter(|t| (self.playback_start..=self.playback_end).contains(t)) {
                        let x = scrub.rect.left() + rail + (t - self.playback_start) / span * width;
                        ui.painter().line_segment(
                            [egui::pos2(x, scrub.rect.top()), egui::pos2(x, scrub.rect.top() + 6.0)],
                            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 60)),
                        );
                    }
                }
                if scrub.changed() {
                    self.dirty = true;
                }
            });
            let typing = ui.ctx().wants_keyboard_input();
            let step = ui.input(|i| {
                (i.key_pressed(egui::Key::ArrowRight) as i32) - (i.key_pressed(egui::Key::ArrowLeft) as i32)
            });
            if step != 0 && !typing {
                self.animate = false;
                self.time = (self.time + step as f32 / sims::SIM_FPS).clamp(self.playback_start, self.playback_end);
                self.dirty = true;
            }

            ui.horizontal(|ui| {
                let range_changed = pfield(ui, pdrag(&mut self.playback_start).speed(0.1).prefix("Start ")).changed()
                    | pfield(ui, pdrag(&mut self.playback_end).speed(0.1).prefix("End   ")).changed();
                if self.playback_end < self.playback_start {
                    self.playback_end = self.playback_start;
                }
                if range_changed {
                    self.time = self.time.clamp(self.playback_start, self.playback_end);
                    self.dirty = true;
                }
                ui.checkbox(&mut self.pause_on_edit, "Pause on edit").on_hover_text("Pause playback whenever the scene is edited: parameters, wiring, flags or gizmo drags");
            });
        });
    }

    // Node Info: the node itself (type, flags, inputs, cook time, errors), then what its
    // geometry holds.
    fn node_info_ui(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let title = node.title(id);
        ui.heading(&title);
        let mut rows: Vec<(String, String)> = vec![("Type".into(), node.type_label())];
        let path = self.graph.path_in_object(id);
        if let Some(object) = self.graph.enclosing_geometry(id) {
            rows.push(("Path".into(), format!("{} / {path}", self.graph.nodes[&object].title(object))));
        }
        let mut flags = Vec::new();
        if self.graph.is_display(id) {
            flags.push("display");
        }
        if node.bypass {
            flags.push("bypassed");
        }
        if self.graph.time_dependent(id) {
            flags.push("time dependent");
        }
        if !flags.is_empty() {
            rows.push(("Flags".into(), flags.join(", ")));
        }
        let inputs = node.kind.num_inputs();
        if inputs > 0 || node.kind.is_multi_input() {
            let connected = self.graph.links.iter().filter(|l| l.to == id).count();
            rows.push(("Inputs".into(), if node.kind.is_multi_input() { format!("{connected} connected (any number)") } else { format!("{connected} of {inputs} connected") }));
        }
        if let Some(stat) = perf::get(id) {
            rows.push(("Cook".into(), format!("{} last, {} on average, {} cooks", perf::ms(stat.last), perf::ms(stat.average()), stat.cooks)));
        }
        egui::Grid::new(("node_info_node", id)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
            for (label, value) in &rows {
                ui.weak(label);
                ui.label(value);
                ui.end_row();
            }
        });
        if let Some(error) = node_errors(|e| e.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
        }
        // Geometry nodes: what they make at this frame.
        let makes_geometry = !matches!(node.context, AppContext::Out | AppContext::Tasks | AppContext::Img | AppContext::Mat) || node.kind.is_object();
        if !makes_geometry {
            return;
        }
        let fresh = self.info_mesh.as_ref().is_some_and(|(n, t, _)| *n == id && *t == self.time);
        if !fresh {
            let mut cache = self.valid_cache(&self.graph.cook_fingerprints(scene_fingerprint(&self.graph.serialize())));
            let mesh = self.graph.cook(id, self.time, &mut cache);
            self.info_mesh = Some((id, self.time, mesh));
        }
        let Some((_, _, mesh)) = self.info_mesh.as_ref() else { return };
        for section in nodeinfo::summarize(mesh) {
            ui.add_space(6.0);
            ui.strong(section.title);
            egui::Grid::new(("node_info", id, section.title)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                for (label, value) in &section.rows {
                    ui.weak(label);
                    ui.add(egui::Label::new(value).wrap());
                    ui.end_row();
                }
            });
        }
        let warnings = mesh.validation_warnings();
        if !warnings.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", warnings.join(" · ")));
        }
    }

    // The performance monitor: each node's cook times, slowest first.
    fn performance_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut recording = perf::recording();
            let label = if recording { "⏺ Recording" } else { "⏸ Paused" };
            if ui.toggle_value(&mut recording, label).on_hover_text("Time every node cook").changed() {
                perf::set_recording(recording);
            }
            if ui.button("Clear").on_hover_text("Forget the times so far").clicked() {
                perf::clear();
            }
            ui.checkbox(&mut self.show_cook_times, "Show cook times on nodes");
            let dt = self.frame_time.max(1e-4);
            ui.weak(format!("Interface: {:.0} fps ({:.1} ms a frame)", 1.0 / dt, dt * 1000.0));
        });
        let mut stats: Vec<(NodeId, perf::Stat)> = perf::snapshot().into_iter().filter(|(id, _)| self.graph.nodes.contains_key(id)).collect();
        if stats.is_empty() {
            ui.add_space(8.0);
            ui.weak("Nothing has cooked since the times were cleared. Change a parameter, or play the timeline, and each node's cook time shows here.");
            return;
        }
        let total: f64 = stats.iter().map(|(_, s)| s.total).sum();
        let cooks: u64 = stats.iter().map(|(_, s)| s.cooks).sum();
        ui.label(format!("{} nodes cooked {cooks} times: {} of their own work in all.", stats.len(), perf::ms(total)));
        ui.weak("A node's own time leaves out the time its inputs took, so the slowest node is the one to look at. Click a node to go to it.");
        match self.perf_sort {
            1 => stats.sort_by(|a, b| b.1.last.total_cmp(&a.1.last)),
            2 => stats.sort_by(|a, b| b.1.average().total_cmp(&a.1.average())),
            3 => stats.sort_by(|a, b| b.1.max.total_cmp(&a.1.max)),
            4 => stats.sort_by(|a, b| b.1.cooks.cmp(&a.1.cooks)),
            _ => {}
        }
        let mut jump = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("performance_grid").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.strong("Node");
                ui.strong("Type");
                for (column, label) in [(4, "Cooks"), (1, "Last"), (2, "Average"), (3, "Slowest"), (0, "Total")] {
                    let text = if self.perf_sort == column { format!("{label} ▼") } else { label.to_string() };
                    if ui.add(egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click())).on_hover_text("Sort by this").clicked() {
                        self.perf_sort = column;
                    }
                }
                ui.strong("Share");
                ui.end_row();
                for (id, stat) in &stats {
                    let node = &self.graph.nodes[id];
                    let path = match self.graph.enclosing_geometry(*id) {
                        Some(object) => format!("{} / {}", self.graph.nodes[&object].title(object), self.graph.path_in_object(*id)),
                        None => node.title(*id),
                    };
                    if ui.link(path).on_hover_text("Go to this node").clicked() {
                        jump = Some(*id);
                    }
                    ui.label(node.type_label());
                    ui.label(stat.cooks.to_string());
                    ui.label(perf::ms(stat.last)).on_hover_text(format!("{} including its inputs", perf::ms(stat.last_with_inputs)));
                    ui.label(perf::ms(stat.average()));
                    ui.label(perf::ms(stat.max));
                    ui.label(perf::ms(stat.total));
                    let share = if total > 0.0 { (stat.total / total) as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(share).desired_width(90.0).text(format!("{:.0}%", share * 100.0)));
                    ui.end_row();
                }
            });
        });
        if let Some(id) = jump {
            let (parent, context) = self.graph.nodes.get(&id).map(|n| (n.parent, n.context)).unwrap_or((None, AppContext::Obj));
            self.enter_network(parent);
            if parent.is_none() {
                self.context = context;
            }
            self.selected = Some(id);
        }
    }

    fn spreadsheet_pane(&mut self, ui: &mut egui::Ui) {
        let source_node = if self.sheet_from_selected { self.selected } else { None };
        if let Some(id) = source_node {
            let fresh = matches!(&self.sheet_cache, Some((cached, generation, _)) if *cached == id && *generation == self.cook_generation);
            if !fresh {
                let mut cache = HashMap::new();
                let mesh = self.graph.cook(id, self.time, &mut cache);
                self.sheet_cache = Some((id, self.cook_generation, mesh));
            }
        }
        let source_label = match source_node {
            Some(id) => self
                .graph
                .nodes
                .get(&id)
                .map(|node| node.title(id))
                .unwrap_or_else(|| format!("node {id}")),
            None if self.sheet_from_selected => "no node selected".into(),
            None => "final geometry in the viewport".into(),
        };
        // Borrow the mesh out of self for the frame instead of cloning it.
        let mesh = match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => std::mem::take(cached),
            _ if self.sheet_from_selected => Mesh::default(),
            _ => std::mem::take(&mut self.current_mesh),
        };
        self.spreadsheet_ui(ui, &mesh, source_node.is_none() && !self.sheet_from_selected, &source_label);
        match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => *cached = mesh,
            _ if self.sheet_from_selected => {}
            _ => self.current_mesh = mesh,
        }
    }

    // `selectable`: rows map to the viewport geometry, so clicking them selects components.
    fn spreadsheet_ui(&mut self, ui: &mut egui::Ui, mesh: &Mesh, selectable: bool, source_label: &str) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Geometry");
            ui.selectable_value(&mut self.sheet_from_selected, false, "Displayed output");
            ui.selectable_value(&mut self.sheet_from_selected, true, "Selected node")
                .on_hover_text("Inspect the output of the node selected in the network, e.g. a wrangle mid-chain");
            ui.weak(source_label);
        });
        ui.horizontal_wrapped(|ui| {
            for domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                let label = match domain {
                    SheetDomain::Points => format!("Points ({})", mesh.vertices.len()),
                    SheetDomain::Vertices => format!("Vertices ({})", mesh.indices.len() * 3),
                    SheetDomain::Primitives => format!("Primitives ({})", mesh.indices.len()),
                    SheetDomain::Detail => "Detail".to_string(),
                };
                ui.selectable_value(&mut self.sheet_domain, domain, label);
            }
        });

        let domain = self.sheet_domain;
        let columns = sheet_columns(mesh, domain);
        let normals = if columns.iter().any(|c| matches!(c.source, SheetSource::ComputedNormal(_))) {
            mesh.compute_normals()
        } else {
            Vec::new()
        };
        let count = domain.count(mesh);
        let polys = SheetPolys::new(mesh);
        let selection: Option<&Vec<usize>> = match domain {
            SheetDomain::Points if selectable => Some(&self.selected_vertices),
            SheetDomain::Primitives if selectable => Some(&self.selected_triangles),
            _ => None,
        };
        // Selected primitives are triangles; their rows are polygons.
        let selected: HashSet<usize> = selection
            .map(|s| s.iter().map(|&i| if domain == SheetDomain::Primitives { polys.of.get(i).copied().unwrap_or(i) } else { i }).collect())
            .unwrap_or_default();
        let filter = self.sheet_selected_only && selection.is_some();
        let rows: Vec<usize> = if domain == SheetDomain::Detail {
            if columns.is_empty() { Vec::new() } else { vec![0] }
        } else if filter {
            let mut rows: Vec<usize> = selected.iter().copied().filter(|i| *i < count).collect();
            rows.sort_unstable();
            rows
        } else {
            (0..count).collect()
        };

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(selection.is_some(), |ui| {
                ui.checkbox(&mut self.sheet_selected_only, "Selected only")
                    .on_hover_text("Points and primitives of the displayed output; click rows to select them (Shift adds)");
            });
            if ui.add_enabled(!rows.is_empty(), egui::Button::new("Copy CSV")).on_hover_text("Copy the rows shown as comma-separated values").clicked() {
                let quote = |text: &str| if text.contains(',') || text.contains('"') { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() };
                let mut csv = columns.iter().map(|c| quote(&c.header)).collect::<Vec<_>>().join(",");
                for &row in &rows {
                    csv.push('\n');
                    csv.push_str(&columns.iter().map(|c| quote(&sheet_cell(mesh, &normals, &polys, &c.source, row))).collect::<Vec<_>>().join(","));
                }
                ui.ctx().copy_text(csv);
                self.status = format!("Copied {} rows", rows.len());
            }
            if let Some((lo, hi)) = mesh.bounds() {
                ui.weak(format!("Bounds ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z));
            }
            let warnings = mesh.validation_warnings();
            if !warnings.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", warnings.join(" · ")));
            }
        });

        egui::CollapsingHeader::new("Attribute summary").id_salt("sheet_summary").show(ui, |ui| {
            egui::Grid::new("sheet_summary_grid").striped(true).num_columns(4).show(ui, |ui| {
                for header in ["Attribute", "Class", "Size", "Range"] {
                    ui.strong(header);
                }
                ui.end_row();
                for summary_domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                    let class = match summary_domain {
                        SheetDomain::Points => "point",
                        SheetDomain::Vertices => "vertex",
                        SheetDomain::Primitives => "primitive",
                        SheetDomain::Detail => "detail",
                    };
                    for attribute in sheet_attributes(mesh, summary_domain) {
                        let values = &mesh.point_attributes[&attribute.key];
                        let ranges = (0..attribute.components.min(4))
                            .map(|c| {
                                let lane = values.iter().skip(c).step_by(attribute.components);
                                let (lo, hi) = lane.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                                if lo > hi { "-".to_string() } else { format!("{lo:.3} … {hi:.3}") }
                            })
                            .collect::<Vec<_>>()
                            .join("   ");
                        ui.monospace(&attribute.name);
                        ui.label(class);
                        ui.label(format!("{}{}", attribute.components, if attribute.size_ok { "" } else { " ⚠" }));
                        ui.monospace(ranges);
                        ui.end_row();
                    }
                }
            });
        });
        ui.separator();

        if rows.is_empty() {
            ui.weak(match domain {
                SheetDomain::Detail => "No detail attributes. Create one with a Detail wrangle, e.g. f@total = npoints(0);",
                _ if filter => "Nothing selected.",
                _ => "No elements.",
            });
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        let mut clicked = None;
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            let mut table = egui_extras::TableBuilder::new(ui)
                .id_salt(("geometry_sheet", domain as u8, columns.len()))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(f32::INFINITY);
            for (i, column) in columns.iter().enumerate() {
                table = table.column(if i == 0 && matches!(column.source, SheetSource::Index) {
                    egui_extras::Column::auto().at_least(44.0)
                } else {
                    egui_extras::Column::initial(74.0).at_least(36.0).clip(true)
                });
            }
            table
                .header(20.0, |mut header| {
                    for column in &columns {
                        header.col(|ui| {
                            ui.strong(&column.header).on_hover_text(&column.tooltip);
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows.len(), |mut row| {
                        let element = rows[row.index()];
                        row.set_selected(selected.contains(&element));
                        for column in &columns {
                            row.col(|ui| {
                                ui.label(egui::RichText::new(sheet_cell(mesh, &normals, &polys, &column.source, element)).monospace());
                            });
                        }
                        if row.response().clicked() {
                            clicked = Some(element);
                        }
                    });
                });
        });

        if let (Some(element), true) = (clicked, selection.is_some()) {
            let (list, mode) = match domain {
                SheetDomain::Points => (&mut self.selected_vertices, SelectionMode::Vertex),
                _ => (&mut self.selected_triangles, SelectionMode::Triangle),
            };
            // A polygon row selects all its triangles.
            let members: Vec<usize> = match domain {
                SheetDomain::Primitives => polys.runs.get(element).map_or_else(Vec::new, |run| run.clone().collect()),
                _ => vec![element],
            };
            if shift {
                if selected.contains(&element) {
                    list.retain(|i| !members.contains(i));
                } else {
                    list.extend(members);
                }
            } else {
                *list = members;
            }
            if self.selection_mode != mode {
                self.selection_mode = mode;
                self.selected_edges.clear();
                match mode {
                    SelectionMode::Vertex => self.selected_triangles.clear(),
                    _ => self.selected_vertices.clear(),
                }
            }
        }
    }

    // Writes a File Export node's input at the current time, or every timeline frame ($F in the name).
    fn write_file_export(&mut self, id: NodeId, range: bool) -> Result<String, String> {
        let Some(NodeKind::FileExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return Err("not a File Export node".into());
        };
        let source = self.graph.input_of(id, 0).ok_or("connect geometry to the input")?;
        let times = if range {
            frame_times(self.playback_start.max(0.0), self.playback_end, 1.0 / sims::SIM_FPS)
        } else {
            vec![self.time]
        };
        // Alembic keeps every frame in one file.
        if fileio::extension(file.trim()) == "abc" && !fileio::has_frame_token(file.trim()) {
            let frames: Vec<(f32, Mesh)> = times.iter().map(|t| (*t, self.graph.cook_fresh(Some(source), *t))).collect();
            let path = file.trim();
            if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            abc::write(path, &frames, sims::SIM_FPS).map_err(|e| format!("{path}: {e}"))?;
            return Ok(format!("Wrote {path} ({} frame{})", frames.len(), if frames.len() == 1 { "" } else { "s" }));
        }
        let mut written = Vec::new();
        for time in &times {
            let mesh = self.graph.cook_fresh(Some(source), *time);
            let path = fileio::expand_frame(file.trim(), *time, sims::SIM_FPS);
            write_geometry_file(&mesh, &path).map_err(|e| format!("{path}: {e}"))?;
            written.push(path);
        }
        Ok(match written.as_slice() {
            [one] => format!("Wrote {one}"),
            many => format!("Wrote {} files ({} … {})", many.len(), many[0], many[many.len() - 1]),
        })
    }

    // Applies the animation at the current time; an animated camera being looked through drives the view.
    fn begin_animation_frame(&mut self, ctx: &egui::Context) {
        self.graph.apply_animation(self.time);
        if let Some(request) = self.look_through_request.take() {
            self.look_through(request);
        }
        if self.looking_through {
            // Anything that moved the view since the camera set it (orbit, pan, zoom,
            // framing) leaves the camera; otherwise the camera, edits and animation
            // included, keeps driving the view.
            let moved = self.camera_view.is_some_and(|set| set.iter().zip(self.view_state()).any(|(a, b)| (a - b).abs() > 1e-5));
            let camera = self.active_camera;
            if moved || !camera.is_some_and(|c| self.view_from_camera(c)) {
                self.look_through(None);
                if moved {
                    self.status = "Left the camera's view (the camera is unchanged)".into();
                }
            } else {
                self.camera_view = Some(self.view_state());
            }
        } else if let Some(camera) = self.active_camera.filter(|c| self.graph.is_animated(*c)) {
            self.view_from_camera(camera);
        }
        self.anim_snapshot = self
            .selected
            .and_then(|id| self.graph.nodes.get(&id).map(|n| (id, n.kind.params(), n.xform)));
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::K) && !i.modifiers.command) {
            self.key_selected();
        }
    }

    // Keys this frame's edits: animated parameters always, anything else with Auto Key.
    fn end_animation_frame(&mut self) {
        let mut changed = self.graph.commit_animation_edits(self.time, self.auto_key);
        if changed && !self.auto_key && !self.graph.pending.is_empty() {
            self.status = "Changed without a key (Auto Key is off): press K to key it, or it goes back to the animation on another frame".into();
        }
        if self.auto_key {
            if let Some((id, params, xform)) = self.anim_snapshot.take() {
                let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
                for (target, _) in targets {
                    let channel_id = ChannelId { node: id, target };
                    if self.graph.channels.contains_key(&channel_id) {
                        continue;
                    }
                    let before = match target {
                        ChannelTarget::Param(i) => params.get(i).copied(),
                        ChannelTarget::Xform(i) => Some(xform_component(&xform, i)),
                    };
                    let now = self.graph.channel_value(channel_id);
                    if let (Some(before), Some(now)) = (before, now) {
                        if (now - before).abs() > 1e-4 * before.abs().max(1.0) {
                            self.graph.set_key(channel_id, self.time);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    fn frame_number(&self) -> i64 {
        (self.time * sims::SIM_FPS).round() as i64 + 1
    }

    // Keys the selected node at the current time: an object's transform, or all its animatable parameters.
    fn key_selected(&mut self) {
        let Some(id) = self.selected else {
            self.status = "Select a node to key".into();
            return;
        };
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let mut targets = node.kind.animatable();
        if matches!(node.kind, NodeKind::Light { .. } | NodeKind::Camera { .. }) {
            targets.truncate(6);
        }
        if targets.is_empty() {
            self.status = format!("{} has no parameters that can be animated", node.title(id));
            return;
        }
        let label = node.title(id);
        for (target, _) in &targets {
            self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
        }
        self.dirty = true;
        self.status = format!("Keyed {} channels of {label} at frame {}", targets.len(), self.frame_number());
    }

    fn graph_editor_pane(&mut self, ui: &mut egui::Ui) {
        // The selected node's curves when it is animated, otherwise every animated channel.
        let focus = self.selected.filter(|id| self.graph.is_animated(*id));
        let mut wanted: HashMap<ChannelId, (String, String)> = HashMap::new();
        for channel_id in self.graph.channels.keys() {
            let Some(node) = self.graph.nodes.get(&channel_id.node) else { continue };
            if focus.is_some_and(|f| f != channel_id.node) {
                continue;
            }
            let group = node.title(channel_id.node);
            let name = node
                .kind
                .animatable()
                .into_iter()
                .find(|(t, _)| *t == channel_id.target)
                .map_or_else(|| format!("{:?}", channel_id.target), |(_, n)| n);
            wanted.insert(*channel_id, (format!("{group} · {name}"), group));
        }
        let mut rows: Vec<anim::ChannelRow> = self
            .graph
            .channels
            .iter_mut()
            .filter_map(|(id, channel)| wanted.get(id).map(|(label, group)| anim::ChannelRow { label: label.clone(), group: group.clone(), channel }))
            .collect();
        let result = self.graph_editor.ui_inside(ui, &mut rows, self.time, (self.playback_start, self.playback_end), sims::SIM_FPS);
        drop(rows);
        if result.changed {
            self.graph.channels.retain(|_, channel| !channel.is_empty());
            self.dirty = true;
        }
        if let Some(t) = result.scrub {
            self.time = t.clamp(self.playback_start, self.playback_end.max(self.playback_start));
            self.animate = false;
            self.dirty = true;
        }
    }

    // Eye and look-at point of the viewport camera, for Camera LOPs.
    fn viewport_eye_target(&self) -> ([f32; 3], [f32; 3]) {
        let (sx, cx) = self.cam_rot_x.sin_cos();
        let (sy, cy) = self.cam_rot_y.sin_cos();
        let right = Vec3::new(cy, 0.0, -sy);
        let up = Vec3::new(-sx * sy, cx, -sx * cy);
        let forward = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = self.cam_zoom / self.viewport_focal_px.max(1.0);
        let target = self.cam_target.sub(right.scale(self.cam_pan_x * pan_scale)).add(up.scale(self.cam_pan_y * pan_scale));
        let eye = target.sub(forward.scale(self.cam_zoom));
        ([eye.x, eye.y, eye.z], [target.x, target.y, target.z])
    }

    // Prim hierarchy of a LOP's stage, with the picked prim's properties.
    fn scene_graph_ui(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let fresh = matches!(&self.lop_stage_cache, Some((node, generation, _)) if *node == id && *generation == self.cook_generation);
        if !fresh {
            let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
            self.lop_stage_cache = Some((id, self.cook_generation, stage));
        }
        let Some((_, _, stage)) = self.lop_stage_cache.clone() else { return };
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Scene Graph");
            ui.weak(format!("{} prims", stage.prims.len()));
        });
        egui::ScrollArea::vertical().id_salt("scene_graph").max_height(240.0).show(ui, |ui| {
            if stage.prims.is_empty() {
                ui.weak("Empty stage. Add SOP Import, Reference or Primitive nodes.");
            }
            let roots: Vec<String> = stage.prims.keys().filter(|p| usd::parent_path(p) == Some("/")).cloned().collect();
            for root in roots {
                self.prim_tree_row(ui, &stage, &root, 0);
            }
        });
        let Some(path) = self.stage_selected_prim.clone() else { return };
        let Some(prim) = stage.prims.get(&path) else { return };
        ui.separator();
        egui::Grid::new("prim_properties").num_columns(2).striped(true).show(ui, |ui| {
            ui.weak("Path");
            ui.horizontal(|ui| {
                ui.monospace(&path);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(path.clone());
                }
            });
            ui.end_row();
            ui.weak("Type");
            ui.label(&prim.type_name);
            ui.end_row();
            let world = stage.world_xform(&path);
            ui.weak("World position");
            ui.monospace(format!("{:.3}, {:.3}, {:.3}", world[3][0], world[3][1], world[3][2]));
            ui.end_row();
            ui.weak("State");
            ui.label(match (prim.active, prim.visible) {
                (false, _) => "inactive",
                (true, false) => "invisible",
                _ => "active, visible",
            });
            ui.end_row();
            if let Some(mesh) = &prim.mesh {
                if prim.type_name == "Mesh" {
                    ui.weak("Geometry");
                    ui.label(format!("{} points, {} primitives", mesh.vertices.len(), poly::count(mesh)));
                    ui.end_row();
                    let mut names: Vec<&String> = mesh.point_attributes.keys().filter(|k| !poly::is_internal(k)).collect();
                    names.sort();
                    ui.weak("Primvars");
                    ui.label(names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", "));
                    ui.end_row();
                }
            }
            if let Some(material) = stage.bound_material(&path) {
                ui.weak("Material");
                ui.monospace(material);
                ui.end_row();
            }
            for (name, value) in &prim.attrs {
                ui.weak(name);
                ui.monospace(match value {
                    usd::AttrValue::Float(f) => format!("{f:.4}"),
                    usd::AttrValue::Vec3(v) => format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2]),
                    usd::AttrValue::Token(t) => t.clone(),
                    usd::AttrValue::Bool(b) => b.to_string(),
                });
                ui.end_row();
            }
        });
    }

    fn prim_tree_row(&mut self, ui: &mut egui::Ui, stage: &usd::Stage, path: &str, depth: usize) {
        let Some(prim) = stage.prims.get(path) else { return };
        let children: Vec<String> = stage.children(path).map(|(p, _)| p.clone()).collect();
        let selected = self.stage_selected_prim.as_deref() == Some(path);
        let mut text = egui::RichText::new(format!("{}  ", usd::prim_name(path)));
        if !prim.active {
            text = text.strikethrough().weak();
        } else if !prim.visible {
            text = text.weak();
        }
        let mut row = |ui: &mut egui::Ui, this: &mut Self| {
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, text.clone()).clicked() {
                    this.stage_selected_prim = Some(path.to_string());
                }
                ui.weak(&prim.type_name);
            });
        };
        if children.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                row(ui, self);
            });
            return;
        }
        let state_id = ui.make_persistent_id(("scene_graph_prim", path));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, depth < 2)
            .show_header(ui, |ui| row(ui, self))
            .body(|ui| {
                for child in &children {
                    self.prim_tree_row(ui, stage, child, depth + 1);
                }
            });
    }

    // Menu bar for occasional commands, plus one row of the controls used all the time.
    // The menus, also reachable from one ☰ button when the window is too narrow for them.
    const MENUS: [&'static str; 7] = ["File", "Edit", "Select", "View", "Render", "Windows", "Help"];

    fn menu_contents(&mut self, ui: &mut egui::Ui, name: &str) {
        match name {
            "File" => self.file_menu(ui),
            "Edit" => self.edit_menu(ui),
            "Select" => self.select_menu(ui),
            "View" => self.view_menu(ui),
            "Render" => self.render_menu(ui),
            "Windows" => self.windows_menu(ui),
            _ => self.help_menu(ui),
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let menu_ctx = ui.ctx().clone();
        let shortcut = |s: &egui::KeyboardShortcut| menu_ctx.format_shortcut(s);
        if ui.add(egui::Button::new("Open…").shortcut_text(shortcut(&SHORTCUT_OPEN))).clicked() {
            self.open_scene_dialog();
            ui.close();
        }
        ui.add_enabled_ui(!self.recent_scenes.is_empty(), |ui| {
            ui.menu_button("Open Recent", |ui| {
                let mut chosen = None;
                for path in &self.recent_scenes {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                    let button = ui
                        .add_enabled(path.exists(), egui::Button::new(name))
                        .on_hover_text(path.display().to_string())
                        .on_disabled_hover_text(format!("{} (not found)", path.display()));
                    if button.clicked() {
                        chosen = Some(path.clone());
                    }
                }
                ui.separator();
                if ui.button("Clear Recent").clicked() {
                    self.recent_scenes.clear();
                    save_recent_scenes(&self.recent_scenes);
                    ui.close();
                }
                if let Some(path) = chosen {
                    self.open_scene(path);
                    ui.close();
                }
            });
        });
        ui.menu_button("New from Preset", |ui| {
            for (three_d, heading) in [(false, "Starter effects for 2D games (sprite sheets)"), (true, "For 3D games (vertex animation)")] {
                if three_d {
                    ui.separator();
                }
                ui.weak(heading);
            for preset in presets::PRESETS.iter().filter(|p| p.vertex_animation == three_d) {
                let label = if preset.looping { format!("{} (loop)", preset.name) } else { preset.name.to_owned() };
                if ui.button(label).on_hover_text(preset.description).clicked() {
                    self.load_preset(preset);
                    ui.close();
                }
            }
            }
        });
        ui.separator();
        let save_hover = match &self.scene_path {
            Some(path) => format!("Save to {}", path.display()),
            None => "Choose where to save this scene".to_owned(),
        };
        if ui.add(egui::Button::new("Save").shortcut_text(shortcut(&SHORTCUT_SAVE))).on_hover_text(save_hover).clicked() {
            self.save_scene();
            ui.close();
        }
        if ui.add(egui::Button::new("Save As…").shortcut_text(shortcut(&SHORTCUT_SAVE_AS))).clicked() {
            self.save_scene_dialog();
            ui.close();
        }
        ui.separator();
        if ui.button("Export OBJ…").on_hover_text("Writes the displayed mesh to an OBJ file you choose").clicked() {
            ui.close();
            self.export_mesh_dialog("OBJ", "obj");
        }
        if ui.button("Export STL…").on_hover_text("Writes the displayed mesh to an STL file you choose").clicked() {
            ui.close();
            self.export_mesh_dialog("STL", "stl");
        }
    }

    // Asks where to write the displayed mesh, named after the scene and beside it by default.
    fn export_mesh_dialog(&mut self, format: &str, extension: &str) {
        let stem = self.scene_path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "procedural_output".to_owned(), |s| s.to_string_lossy().into_owned());
        let mut dialog = FileDialog::new().set_title(&format!("Export {format}")).set_file_name(&format!("{stem}.{extension}")).add_filter(&format!("{format} mesh"), &[extension]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            self.status = "Export canceled".into();
            return;
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let file = path.to_string_lossy().into_owned();
        let result = if extension == "obj" { self.current_mesh.export_obj(&file) } else { self.current_mesh.export_stl(&file) };
        self.status = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
            self.do_undo();
            ui.close();
        }
        if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z")).clicked() {
            self.do_redo();
            ui.close();
        }
    }

    fn select_menu(&mut self, ui: &mut egui::Ui) {
        let actions: [(&str, fn(&mut Self)); 9] = [
            ("All", Self::select_all_components),
            ("None", Self::clear_component_selection),
            ("Invert", Self::invert_component_selection),
            ("Grow", Self::grow_component_selection),
            ("Shrink", Self::shrink_component_selection),
            ("Boundary edges", Self::select_boundary_edges),
            ("Non-manifold edges", Self::select_non_manifold_edges),
            ("Isolated points", Self::select_isolated_vertices),
            ("Connected", Self::select_connected_components),
        ];
        for (i, (label, action)) in actions.into_iter().enumerate() {
            if i == 5 {
                ui.separator();
            }
            if ui.button(label).clicked() {
                action(self);
                ui.close();
            }
        }
        ui.separator();
        ui.checkbox(&mut self.isolate_selection, "Isolate selection");
        ui.weak("Shift-drag: box select • double-click an edge: loop");
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.show_points, "Points");
        ui.checkbox(&mut self.show_floor, "Floor grid");
        ui.checkbox(&mut self.show_gizmos, "Gizmos");
        ui.checkbox(&mut self.orthographic, "Orthographic");
        ui.separator();
        for (label, rot_x, rot_y) in [
            ("Top", -1.52, 0.0),
            ("Front", 0.0, 0.0),
            ("Right", 0.0, std::f32::consts::FRAC_PI_2),
            ("Perspective", 0.4, -0.6),
        ] {
            if ui.button(format!("{label} view")).clicked() {
                self.set_camera_view(rot_x, rot_y);
                ui.close();
            }
        }
        if ui.add(egui::Button::new("Frame all").shortcut_text("F")).clicked() {
            self.frame_all();
            ui.close();
        }
        ui.separator();
        pfield(ui, pslider(&mut self.viewport_bg, 15..=80).text("Background"));
        pfield(ui, pslider(&mut self.grid_spacing, 0.1..=2.0).text("Grid spacing"));
        pfield(ui, pslider(&mut self.grid_extent, 1.0..=20.0).text("Grid extent"));
        ui.menu_button("Camera & lights", |ui| {
            pfield(ui, pslider(&mut self.camera_focal, 0.5..=2.5).text("Focal scale"));
            pfield(ui, pslider(&mut self.camera_near, 0.05..=2.0).text("Near clip"));
            ui.separator();
            pfield(ui, pslider(&mut self.ambient_light, 0.0..=1.0).text("Ambient")).on_hover_text(
                "Soft light everywhere, always added, with or without lights in the scene, so the sides facing away from them aren't black. \
                 MoonRay renders get it too, as a dim environment light. Set it to 0 to light the scene only with its own lights.",
            );
            ui.label("Default lights (used when the scene has no lights)");
            vec3_ui(ui, "Key direction", &mut self.key_light_dir, 0.05);
            pfield(ui, pslider(&mut self.key_light_intensity, 0.0..=2.0).text("Key intensity"));
            vec3_ui(ui, "Fill direction", &mut self.fill_light_dir, 0.05);
            pfield(ui, pslider(&mut self.fill_light_intensity, 0.0..=2.0).text("Fill intensity"));
        });
    }

    fn render_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Render with MoonRay").clicked() {
            self.start_moonray_render();
            ui.close();
        }
        let realtime = ui
            .add_enabled(self.gpu_viewport.is_some() && self.flipbook_recording.is_none(), egui::Button::new("Render Real-Time"))
            .on_hover_text("Renders like the viewport, in seconds, through the render camera at the Render Settings size (a sequence if that's ticked there): transparent PNGs, no MoonRay needed");
        if realtime.clicked() {
            self.start_realtime_render();
            ui.close();
        }
        if ui
            .add_enabled(self.gpu_viewport.is_some(), egui::Button::new("Export Sprite Sheet…"))
            .on_hover_text("Renders the playback range in real time and packs the frames into one PNG, with a JSON file describing the grid, ready for a 2D game")
            .clicked()
        {
            self.sprite_sheet.open = true;
            ui.close();
        }
        if ui
            .button("Export Vertex Animation…")
            .on_hover_text("Bakes a deforming object (cloth, deformers, wobbling shapes) over the playback range into vertex animation textures, with shaders for Godot and Unity: the mesh moves on the GPU in the game")
            .clicked()
        {
            self.vat_window.open = true;
            ui.close();
        }
        if ui.button("✨ AI").on_hover_text("Describe a setup and Claude builds it as nodes").clicked() {
            self.assistant.open = !self.assistant.open;
        }
        if ui.button("Render Settings…").clicked() {
            self.show_moonray_settings = true;
            ui.close();
        }
        if ui.button("Render Viewer").clicked() {
            self.viewer.show();
            ui.close();
        }
        if let Some(helper) = wslrender::helper() {
            ui.separator();
            let busy = self.render_setup.is_some();
            if ui.add_enabled(!busy, egui::Button::new("Set Up Rendering on Windows…")).on_hover_text("Installs WSL containers if needed and downloads MoonRay (a few GB, once)").clicked() {
                self.render_setup = Some(start_render_setup(helper, false));
                self.render_setup_log.clear();
                self.render_setup_needs_admin = false;
                self.render_setup_open = true;
                ui.close();
            }
        }
    }

    fn windows_menu(&mut self, ui: &mut egui::Ui) {
        for pane in Pane::ALL {
            let mut open = self.open_panes.contains(&pane);
            if ui.checkbox(&mut open, pane.title()).changed() {
                self.pane_requests.push(PaneRequest::Show(pane, open));
            }
        }
        ui.separator();
        ui.menu_button("Open in New Window", |ui| {
            for pane in Pane::ALL {
                if ui.button(pane.title()).clicked() {
                    self.pane_requests.push(PaneRequest::PopOut(pane));
                    ui.close();
                }
            }
        });
        ui.menu_button("Layouts", |ui| {
            if ui.button("Save Current Layout…").clicked() {
                self.layout_name_prompt = Some(String::new());
                ui.close();
            }
            let names = saved_layouts();
            if !names.is_empty() {
                ui.separator();
            }
            for name in names {
                ui.horizontal(|ui| {
                    if ui.button(&name).on_hover_text("Switch to this layout").clicked() {
                        self.pane_requests.push(PaneRequest::Load(name.clone()));
                        ui.close();
                    }
                    if ui.small_button("🗑").on_hover_text("Delete this layout").clicked() {
                        self.pane_requests.push(PaneRequest::Delete(name.clone()));
                    }
                });
            }
            ui.separator();
            if ui.button("Reset Layout").on_hover_text("Back to the default arrangement of panes").clicked() {
                self.pane_requests.push(PaneRequest::Reset);
                ui.close();
            }
        });
        ui.weak("Drag tabs to rearrange or split; drag one out to float it.");
        ui.separator();
        if ui.button("Render Viewer").clicked() {
            self.viewer.show();
            ui.close();
        }
        if ui.button("Render Settings").clicked() {
            self.show_moonray_settings = true;
            ui.close();
        }
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("📖 Documentation").on_hover_text("All nodes and VEX, as a web page in your browser").clicked() {
            self.status = match open_documentation() {
                Ok(path) => format!("Opened the documentation ({path})"),
                Err(e) => format!("Couldn't open the documentation: {e}"),
            };
            ui.close_menu();
        }
        ui.separator();
        self.updater.menu(ui);
        ui.separator();
        ui.weak("Houdini is a trademark of Side Effects Software Inc.\nProceduralFX Studio is not affiliated with or endorsed by SideFX.");
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            // Narrower windows drop the title, then fold the menus into one ☰ button.
            let width = ui.available_width();
            let updater = if self.updater.has_indicator() { 190.0 } else { 0.0 };
            if width - updater > 640.0 {
                ui.strong("ProceduralFX Studio");
                ui.separator();
            }
            if width - updater > 470.0 {
                for name in Self::MENUS {
                    ui.menu_button(name, |ui| self.menu_contents(ui, name));
                }
            } else {
                ui.menu_button("☰", |ui| {
                    for name in Self::MENUS {
                        ui.menu_button(name, |ui| self.menu_contents(ui, name));
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.updater.indicator(ui));
        });

        // The toolbar wraps a group at a time when the window is narrow, so every control
        // stays visible; the render controls keep to the right edge when they fit there.
        ui.horizontal_wrapped(|ui| {
            toolbar_group(ui, "contexts", |ui| {
                for (context, label, hover) in [
                    (AppContext::Obj, "OBJ", "Objects and their geometry (SOP) networks"),
                    (AppContext::Mat, "MAT", "Materials"),
                    (AppContext::Out, "OUT", "Render nodes"),
                    (AppContext::Stage, "STAGE", "USD stage (LOP) network"),
                    (AppContext::Img, "IMG", "Compositing (COP networks)"),
                    (AppContext::Tasks, "TASKS", "Task graphs: renders, wedges and batch jobs (TOP networks)"),
                ] {
                    if ui.selectable_label(self.context == context, egui::RichText::new(label).strong()).on_hover_text(hover).clicked()
                        && self.context != context
                    {
                        self.context = context;
                        // An open network belongs to its own context (a Material Builder to MAT).
                        if self.network().is_some_and(|id| self.graph.nodes.get(&id).is_some_and(|n| n.context != context)) {
                            self.current_network = None;
                        }
                        self.selected = None;
                        self.dirty = true;
                    }
                }
            });
            toolbar_group(ui, "shading", |ui| {
                ui.separator();
                ui.selectable_value(&mut self.render_mode, RenderMode::ShadedWireframe, "Shaded");
                ui.selectable_value(&mut self.render_mode, RenderMode::FlatShaded, "Flat");
                ui.selectable_value(&mut self.render_mode, RenderMode::Wireframe, "Wire");
                ui.selectable_value(&mut self.render_mode, RenderMode::Normals, "Normals");
            });
            toolbar_group(ui, "camera", |ui| {
                ui.separator();
                let cameras = self.camera_ids();
                let looking = self.active_camera.filter(|_| self.looking_through);
                let shown = looking.and_then(|id| cameras.iter().find(|(c, _)| *c == id)).map_or("Free view".to_owned(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("look_through").selected_text(format!("👁 {shown}")).show_ui(ui, |ui| {
                    if ui.selectable_label(looking.is_none(), "Free view").clicked() && looking.is_some() {
                        self.look_through_request = Some(None);
                    }
                    for (id, name) in &cameras {
                        if ui.selectable_label(looking == Some(*id), name).clicked() {
                            self.look_through_request = Some(Some(*id));
                        }
                    }
                }).response.on_hover_text("Look through a camera without changing it");
            });
            toolbar_group(ui, "selection", |ui| {
                ui.separator();
                let mode_changed = ui.selectable_value(&mut self.selection_mode, SelectionMode::Vertex, "Points").changed()
                    || ui.selectable_value(&mut self.selection_mode, SelectionMode::Triangle, "Faces").changed()
                    || ui.selectable_value(&mut self.selection_mode, SelectionMode::Edge, "Edges").changed();
                if mode_changed {
                    self.clear_component_selection();
                }
            });
            toolbar_group(ui, "undo", |ui| {
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("↶")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                    self.do_undo();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↷")).on_hover_text("Redo (Ctrl+Shift+Z)").clicked() {
                    self.do_redo();
                }
            });
            toolbar_group(ui, "transform", |ui| {
                ui.separator();
                for (mode, label, hover) in [
                    (GizmoMode::Translate, "✥ Move", "Move the selected object (W)"),
                    (GizmoMode::Rotate, "⟳ Rotate", "Rotate the selected object (E)"),
                    (GizmoMode::Scale, "⤢ Scale", "Scale the selected geometry object (R)"),
                ] {
                    ui.selectable_value(&mut self.gizmo_mode, mode, label).on_hover_text(hover);
                }
            });
            toolbar_group(ui, "keys", |ui| {
                ui.separator();
                if ui.button("◆ Key").on_hover_text("Key the selected node at this frame (K)").clicked() {
                    self.key_selected();
                }
                if !self.graph.pending.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(255, 170, 60), format!("● {} unkeyed", self.graph.pending.len()))
                        .on_hover_text("Changes to animated parameters, not keyed (Auto Key is off). Press K to key them; on another frame they go back to the animation");
                }
                ui.toggle_value(&mut self.auto_key, "Auto Key").on_hover_text("Key every parameter you change, not only animated ones");
                let mut graph_open = self.open_panes.contains(&Pane::GraphEditor);
                if ui.toggle_value(&mut graph_open, "📈 Graph").on_hover_text("Graph Editor").changed() {
                    self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, graph_open));
                }
            });
            if ui.available_size_before_wrap().x < self.toolbar_render_width + 8.0 && ui.cursor().min.x > ui.max_rect().left() + 1.0 {
                ui.end_row();
            }
            if ui.max_rect().width() < self.toolbar_render_width + 8.0 {
                // Narrower than the render controls themselves: left to right, wrapping.
                self.render_controls(ui, false);
            } else {
                let row = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let right = ui.max_rect().right();
                    self.render_controls(ui, true);
                    self.toolbar_render_width = right - ui.min_rect().left();
                });
            }
        });
    }

    // The toolbar's camera, Render and settings controls; `from_right` lays them out
    // right to left (⚙ at the edge).
    fn render_controls(&mut self, ui: &mut egui::Ui, from_right: bool) {
        let settings = |app: &mut Self, ui: &mut egui::Ui| {
            if ui.button("⚙").on_hover_text("Render Settings").clicked() {
                app.show_moonray_settings = true;
            }
        };
        let render = |app: &mut Self, ui: &mut egui::Ui| {
            if app.moonray_rx.is_some() || app.sequence.is_some() {
                if ui.button("■ Cancel").clicked() {
                    app.cancel_render();
                }
                if let Some(started) = app.render_started {
                    ui.label(format!("{:.0}s", started.elapsed().as_secs_f32()));
                }
                ui.spinner();
            } else if ui.button(egui::RichText::new("▶ Render").strong()).on_hover_text("Render with MoonRay").clicked() {
                app.start_moonray_render();
            }
        };
        let camera = |app: &mut Self, ui: &mut egui::Ui| {
            let cameras = app.camera_ids();
            render_camera_combo(ui, &mut app.render_camera, &cameras);
        };
        if from_right {
            settings(self, ui);
            render(self, ui);
            camera(self, ui);
            ui.weak("Camera");
        } else {
            ui.weak("Camera");
            camera(self, ui);
            render(self, ui);
            settings(self, ui);
        }
    }

    fn open_scene_dialog(&mut self) {
        let mut dialog = FileDialog::new().add_filter("ProceduralFX Scene", &[SCENE_EXTENSION, "mhip", "json"]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.pick_file() {
            Some(path) => self.open_scene(path),
            None => self.status = "Load canceled".into(),
        }
    }

    fn open_scene(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(s) if scene_problem(&s).is_some() => {
                self.status = format!("Can't open {}: {}", path.display(), scene_problem(&s).unwrap_or_default());
            }
            Ok(s) => {
                self.undo.push(self.last_state.clone());
                self.use_saved_timeline(&s);
                let unread = unread_nodes(&s, &Graph::deserialize(&s));
                self.load_state(s);
                self.mark_saved();
                self.status = match unread {
                    0 => format!("Loaded {}", path.display()),
                    n => format!(
                        "Loaded {}, but {n} node{} couldn't be read (made by a newer version, or damaged). Saving over the file would lose them.",
                        path.display(),
                        if n == 1 { "" } else { "s" }
                    ),
                };
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => {
                self.status = format!("Load failed: {e}");
                if !path.exists() {
                    self.recent_scenes.retain(|p| p != &path);
                    save_recent_scenes(&self.recent_scenes);
                }
            }
        }
    }

    // Starts a new scene from a starter effect: its nodes, the playback range it's made for,
    // its camera as the render camera, and sprite-sheet settings to suit (a seamless loop
    // for looping effects). Undoable, like opening a file; saving asks where.
    fn load_preset(&mut self, preset: &presets::Preset) {
        self.undo.push(self.last_state.clone());
        self.load_state(preset.scene.to_owned());
        self.scene_path = None;
        self.selected = None;
        self.current_network = None;
        let camera = self.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. })).map(|(id, _)| *id).min();
        self.use_preset_settings(preset, camera);
        let export = if preset.vertex_animation { "Render → Export Vertex Animation… exports it for a 3D game" } else { "Render → Export Sprite Sheet… exports it for a game" };
        self.status = format!("New scene from the {} preset. {export}.", preset.name);
    }

    // A preset's playback range, sprite-sheet settings and, given its camera, render camera.
    fn use_preset_settings(&mut self, preset: &presets::Preset, camera: Option<NodeId>) {
        self.animate = false;
        (self.playback_start, self.playback_end) = preset.range;
        self.time = preset.range.0;
        if let Some(id) = camera {
            self.render_camera = RenderCamera::Node(id);
            // Look through it in the viewport too, so what you see is what exports.
            self.look_through(Some(id));
        }
        // Looping presets loop in the scene itself (Make Loop, or by construction), so the
        // rendered frames need no blending.
        let s = &mut self.sprite_sheet;
        (s.width, s.height, s.seamless) = (preset.frame[0], preset.frame[1], false);
        self.dirty = true;
    }

    // Saves to the current scene file, or asks where to save if there isn't one yet.
    fn save_scene(&mut self) {
        match self.scene_path.clone() {
            // An old .mhip scene is saved as .pfs beside it; the original is left alone.
            Some(path) if path.extension().is_some_and(|e| e == "mhip") => self.write_scene(path.with_extension(SCENE_EXTENSION)),
            Some(path) => self.write_scene(path),
            None => self.save_scene_dialog(),
        }
    }

    fn save_scene_dialog(&mut self) {
        let name = self.scene_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        // Saving an old .mhip scene writes a .pfs next to it.
        let name = name.map(|n| match n.strip_suffix(".mhip") {
            Some(stem) => format!("{stem}.{SCENE_EXTENSION}"),
            None => n,
        });
        let mut dialog = FileDialog::new().set_file_name(name.as_deref().unwrap_or(SCENE_FILE)).add_filter("ProceduralFX Scene", &[SCENE_EXTENSION]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.save_file() {
            Some(path) => self.write_scene(path),
            None => self.status = "Save canceled".into(),
        }
    }

    fn write_scene(&mut self, path: PathBuf) {
        match write_file_atomically(&path, &self.scene_text()) {
            Ok(()) => {
                self.mark_saved();
                if let Some(session) = &mut self.recovery {
                    session.clear_autosave();
                }
                self.status = format!("Saved {}", path.display());
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    // Flipbook: renders the playback range in the viewport, frame by frame.
    fn start_flipbook(&mut self) {
        // The GPU viewport renders the frames itself; screenshots need it in the main window.
        let capture = self.gpu_viewport.is_some().then_some(flipbook::Capture { render: None, save_to: None, sheet: None, preview: None });
        if capture.is_none() && self.viewport_rect.is_none() {
            self.status = "Flipbook: show the Viewport in the main window first".into();
            return;
        }
        let times = frame_times(self.playback_start, self.playback_end, 1.0 / sims::SIM_FPS);
        if times.len() > 1000 {
            self.status = format!("Flipbook: {} frames is too many; shorten the playback range to at most 1000", times.len());
            return;
        }
        self.animate = false;
        self.flipbook_viewer = None;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, capture));
    }

    // Renders the current frame, or Render Settings' sequence, with the GPU renderer:
    // through the render camera at the render size, to transparent PNGs in the temp
    // folder, shown in the Render Viewer. No MoonRay needed.
    fn start_realtime_render(&mut self) {
        if self.gpu_viewport.is_none() {
            self.status = "Real-time rendering needs the GPU viewport (it's off with PFX_RENDERER=opengl or PFX_VIEWPORT=cpu)".into();
            return;
        }
        if self.flipbook_recording.is_some() {
            self.status = "A flipbook or real-time render is already running".into();
            return;
        }
        let opts = &self.moonray_default_opts;
        let times = if opts.sequence { frame_times(opts.range_start, opts.range_end, opts.range_step) } else { vec![self.time] };
        if times.len() > 1000 {
            self.status = format!("Real-time render: {} frames is too many; render at most 1000 at a time", times.len());
            return;
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let dir = std::env::temp_dir().join(format!("pfx_realtime_{stamp}"));
        let size = [self.moonray_default_width.clamp(1, 16384) as u32, self.moonray_default_height.clamp(1, 16384) as u32];
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(flipbook::Capture { render: Some(size), save_to: Some(dir), sheet: None, preview: None })));
    }

    // The playback range's times for a sprite sheet: every Nth frame.
    fn sprite_sheet_times(&self) -> Vec<f32> {
        frame_times(self.playback_start, self.playback_end, self.sprite_sheet.every.max(1) as f32 / sims::SIM_FPS)
    }

    // The geometry object a bake uses by default: the one being viewed, else the first.
    fn vat_default_object(&self) -> Option<NodeId> {
        let objects = self.graph.geometry_ids();
        // Up from the network being viewed to its object.
        let mut at = self.current_network;
        for _ in 0..64 {
            match at.and_then(|n| self.graph.nodes.get(&n)).and_then(|n| n.parent) {
                Some(parent) => at = Some(parent),
                None => break,
            }
        }
        at.filter(|o| objects.contains(o)).or_else(|| objects.first().copied())
    }

    fn vat_window(&mut self, ctx: &egui::Context) {
        // A bake in the background: progress, then what it wrote.
        if let Some(rx) = &self.vat_job {
            loop {
                match rx.try_recv() {
                    Ok(VatMessage::Progress(done, total)) => self.status = format!("Baking vertex animation: frame {done} of {total}"),
                    Ok(VatMessage::Done(result)) => {
                        self.status = match result {
                            Ok(summary) => summary,
                            Err(e) => format!("Couldn't bake the vertex animation: {e}"),
                        };
                        self.vat_job = None;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.vat_job = None;
                        break;
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !self.vat_window.open {
            return;
        }
        let objects: Vec<(NodeId, String)> = self.graph.geometry_ids().into_iter().map(|id| (id, self.graph.nodes[&id].title(id))).collect();
        let default_object = self.vat_default_object();
        let mut open = true;
        let mut export = false;
        let busy = self.vat_job.is_some();
        let every = self.vat_window.every.max(1);
        let count = frame_times(self.playback_start, self.playback_end, every as f32 / sims::SIM_FPS).len();
        own_window(ctx, "export_vat", "Export Vertex Animation", [480.0, 360.0], [380.0, 260.0], &mut open, true, |ui| {
            let w = &mut self.vat_window;
            w.every = w.every.max(1);
            ui.label("Bakes an object's motion over the playback range into vertex animation textures: a mesh (OBJ) and two textures (EXR) of every frame's motion. A shader in the game moves the mesh on the GPU.");
            ui.add_space(4.0);
            egui::Grid::new("vat_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Object");
                let chosen = w.object.filter(|o| objects.iter().any(|(id, _)| id == o)).or(default_object);
                let title = objects.iter().find(|(id, _)| Some(*id) == chosen).map_or("(none)", |(_, t)| t.as_str());
                egui::ComboBox::from_id_salt("vat_object").selected_text(title).show_ui(ui, |ui| {
                    for (id, t) in &objects {
                        if ui.selectable_label(chosen == Some(*id), t.as_str()).clicked() {
                            w.object = Some(*id);
                        }
                    }
                });
                ui.end_row();
                ui.label("Kind");
                let label = |m: Option<vat::Mode>| match m {
                    None => "Automatic",
                    Some(vat::Mode::Soft) => "Soft: points that stay",
                    Some(vat::Mode::Rigid) => "Rigid: pieces",
                    Some(vat::Mode::Fluid) => "Fluid: a changing mesh",
                };
                egui::ComboBox::from_id_salt("vat_mode").selected_text(label(w.mode)).show_ui(ui, |ui| {
                    for (m, hint) in [
                        (None, "The kind that suits the motion: Fluid if the mesh changes, Rigid if it's pieces that keep their shape, else Soft"),
                        (Some(vat::Mode::Soft), "Points that stay the same and move freely: cloth, deformers, wobbling or twisting shapes"),
                        (Some(vat::Mode::Rigid), "Pieces that move and turn without bending: destruction and RBD simulations. Pieces come from the piece attribute, or what's connected"),
                        (Some(vat::Mode::Fluid), "A mesh whose points and polygons change: liquids (Particle Fluid Surface), growing shapes. Bigger textures, and frames don't blend"),
                    ] {
                        ui.selectable_value(&mut w.mode, m, label(m)).on_hover_text(hint);
                    }
                });
                ui.end_row();
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.label("every");
                    ui.add(egui::DragValue::new(&mut w.every).range(1..=100));
                    ui.label(format!("frame{} of the playback range: {count} frame{}", if w.every == 1 { "" } else { "s" }, if count == 1 { "" } else { "s" }));
                });
                ui.end_row();
                ui.label("Also for");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut w.godot, "Godot").on_hover_text("A spatial shader and a ShaderMaterial (name_vat_material.tres), with import settings that keep the textures exact. Put name.obj on a MeshInstance3D and the material on it.");
                    ui.checkbox(&mut w.unity, "Unity").on_hover_text("A URP shader and material (name_vat.mat), with import settings that keep the textures exact. Put name.obj in the scene and the material on it. Needs the Linear colour space (URP's default).");
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && chosen_ok(&objects, default_object, w.object) && count > 0, egui::Button::new("Export…")).clicked() {
                    export = true;
                }
                if busy {
                    ui.spinner();
                    ui.weak("Baking…");
                }
            });
        });
        self.vat_window.open = open;
        if !export {
            return;
        }
        let Some(object) = self.vat_window.object.filter(|o| objects.iter().any(|(id, _)| id == o)).or(default_object) else { return };
        let name = self.graph.nodes.get(&object).map_or_else(|| "vat".to_owned(), |n| n.title(object));
        let Some(path) = FileDialog::new().set_title("Save the vertex animation").set_file_name(&format!("{name}.obj")).add_filter("OBJ mesh (with its textures beside it)", &["obj"]).save_file() else { return };
        self.start_vat_bake(object, path.with_extension(""));
    }

    // Bakes `object` with the window's settings to the files beside `stem`, in the
    // background; vat_window reports progress and the result.
    fn start_vat_bake(&mut self, object: NodeId, stem: PathBuf) {
        let every = self.vat_window.every.max(1);
        let times = frame_times(self.playback_start, self.playback_end, every as f32 / sims::SIM_FPS);
        let fps = sims::SIM_FPS / every as f32;
        let (godot, unity, mode) = (self.vat_window.godot, self.vat_window.unity, self.vat_window.mode);
        let graph = self.graph.clone();
        let (tx, rx) = mpsc::channel();
        self.vat_job = Some(rx);
        self.vat_window.open = false;
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = vat::bake_object(&graph, object, &times, mode, &mut |done, total| {
                let _ = progress_tx.send(VatMessage::Progress(done, total));
            })
            .and_then(|bake| vat::write(&bake, &stem, fps).map(|(files, layout)| (bake.mode, files, layout)))
            .and_then(|(kind, files, layout)| {
                if godot {
                    vat::write_godot(&files, &layout, fps)?;
                }
                if unity {
                    vat::write_unity(&files, &layout, fps)?;
                }
                Ok(format!("Baked as {}: {} elements over {} frames, to {} and its textures", kind.name(), layout.points, layout.frames, files.obj.display()))
            });
            let _ = tx.send(VatMessage::Done(result));
        });
    }

    fn sprite_sheet_window(&mut self, ctx: &egui::Context) {
        if !self.sprite_sheet.open {
            return;
        }
        let mut open = true;
        let (mut export, mut preview) = (false, false);
        let count = self.sprite_sheet_times().len();
        own_window(ctx, "export_sprite_sheet", "Export Sprite Sheet", [500.0, 460.0], [400.0, 320.0], &mut open, true, |ui| {
            let s = &mut self.sprite_sheet;
            ui.label("Renders the playback range through the render camera with the real-time renderer, then packs the frames into one PNG.");
            ui.add_space(4.0);
            egui::Grid::new("sprite_sheet_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Frame size");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.width).range(8..=4096).suffix(" px"));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut s.height).range(8..=4096).suffix(" px"));
                });
                ui.end_row();
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.label("every");
                    ui.add(egui::DragValue::new(&mut s.every).range(1..=100));
                    ui.label(format!("frame{} of the playback range: {count} frame{}, {:.3} fps", if s.every == 1 { "" } else { "s" }, if count == 1 { "" } else { "s" }, sims::SIM_FPS / s.every.max(1) as f32));
                });
                ui.end_row();
                ui.label("Columns");
                ui.horizontal(|ui| {
                    let mut auto = s.columns == 0;
                    if ui.checkbox(&mut auto, "Automatic").on_hover_text("As close to square as the frames allow").changed() {
                        s.columns = if auto { 0 } else { spritesheet::columns(count, [s.width, s.height], 0) };
                    }
                    if !auto {
                        ui.add(egui::DragValue::new(&mut s.columns).range(1..=1000));
                    }
                });
                ui.end_row();
                ui.label("Particles");
                ui.checkbox(&mut s.fill_grid, "Fill the grid").on_hover_text(
                    "For engines' particle systems, which play every cell of the grid: picks a column count that leaves no empty cells (24 frames: 6×4), \
                     so particles never show a blank frame. With Godot ticked, also writes name_particles.tres, a material for GPUParticles2D. \
                     In Unity, use Texture Sheet Animation in Grid mode with the columns and rows (or Sprites mode with the sliced sprites).",
                );
                ui.end_row();
                ui.label("Smooth playback");
                ui.checkbox(&mut s.motion_vectors, "Motion vectors").on_hover_text(
                    "Also writes name_motion.png: how each pixel moves on its way to the next frame. Games then slide each frame towards the next \
                     instead of cross-fading or jumping, so a sheet with a quarter of the frames plays about as smoothly. \
                     bevy_procedural_fx plays it with FxFlipbook; with Godot or Unity ticked, you also get a shader and material that use it.",
                );
                ui.end_row();
                ui.label("Lighting");
                ui.checkbox(&mut s.normal_map, "Normal map").on_hover_text(
                    "Also writes name_normal.png: each frame's shape, read from how opaque it is, so a game's 2D lights light the effect from their side. \
                     Suits smoke, dust and clouds. With Godot ticked, name_sprite.tres (for the AnimatedSprite2D) gives it to Light2D; with Unity ticked, the sliced sprites get it as their \
                     _NormalMap, for URP's 2D lights. bevy_procedural_fx lights an FxFlipbook with FxLight2d lights.",
                );
                ui.end_row();
                ui.label("Glow");
                ui.checkbox(&mut s.glow, "Glow sheet").on_hover_text(
                    "Also writes name_glow.png: the light of glowing parts (fire, sparks, spells), which the sheet itself rolls off towards white. \
                     Games put it back at full brightness, or stronger, so their bloom makes the hot parts glow. bevy_procedural_fx's FxFlipbook adds it (turn on HDR and Bloom on the camera); \
                     with Godot or Unity ticked as well as Motion vectors, their flipbook shaders do too.",
                );
                ui.end_row();
                ui.label("Trim");
                ui.checkbox(&mut s.trim, "Crop empty space").on_hover_text("Crops every frame to the same box: what any frame shows, plus a 1-pixel transparent border. The JSON records where the box was in the full frame.");
                ui.end_row();
                ui.label("Loop");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.seamless, "Seamless").on_hover_text(
                        "Renders a few frames past the end of the playback range and fades those pictures into the first frames, so the last frame leads into the first. \
                         The sheet still has one frame per frame of the range. For simulations, a Make Loop node (Pyro) or Loop Particles node in the network loops better: \
                         it blends the simulation itself rather than two pictures, or for particles loops exactly, and loops in the viewport too.",
                    );
                    if s.seamless {
                        ui.label("blend");
                        ui.add(egui::DragValue::new(&mut s.blend).range(1..=count.max(1) as u32));
                        ui.label(if s.blend == 1 { "frame" } else { "frames" });
                    }
                });
                ui.end_row();
                ui.label("Also for");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.godot, "Godot").on_hover_text("Also write name.tres, a Godot 4 SpriteFrames resource: put it on an AnimatedSprite2D and it plays the sheet, looping, at its frame rate. Export into your Godot project, or copy the PNG and .tres together.");
                    ui.checkbox(&mut s.unity, "Unity").on_hover_text("Also write name.png.meta, so Unity imports the PNG already sliced into one sprite per frame (name_0, name_1...), uncompressed and full size. Export into your project's Assets folder; select the sprites and drag them into the scene to make an animation.");
                });
                ui.end_row();
                ui.label("Live");
                ui.checkbox(&mut s.live, "Keep updating").on_hover_text(
                    "After exporting, export again to the same files whenever the scene, camera or these settings change. \
                     A Bevy game using bevy_procedural_fx with Bevy's file_watcher feature (embedded_watcher if the game builds its art in) then updates as you work.",
                );
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.weak("Uses the camera chosen in the toolbar's render camera menu. Beside the PNG, name.pfx.json gives the frame size, columns and rows, frame count and frame rate. The bevy_procedural_fx crate plays it in Bevy.");
            ui.add_space(4.0);
            let busy = self.flipbook_recording.is_some();
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && count > 0, egui::Button::new("Export…")).clicked() {
                    export = true;
                }
                if ui
                    .add_enabled(!busy && count > 0, egui::Button::new("Preview"))
                    .on_hover_text("Plays the frames as they'll be exported, seamless blend included, looping in the flipbook viewer. Nothing is saved.")
                    .clicked()
                {
                    preview = true;
                }
                if let Some(live) = &self.live_sheet {
                    let name = live.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.weak(format!("Keeping {name} up to date"));
                    if ui.button("Stop").clicked() {
                        self.sprite_sheet.live = false;
                    }
                }
            });
        });
        if preview {
            self.preview_sprite_sheet();
        }
        if export {
            if let Some(path) = FileDialog::new().set_title("Save the sprite sheet").set_file_name("sprite_sheet.png").add_filter("PNG image", &["png"]).save_file() {
                self.live_sheet = None;
                self.start_sprite_sheet(path, false);
            }
        }
        self.sprite_sheet.open = open && !export;
    }

    // The times a sprite sheet renders, with a seamless loop's extra frames past the end, and
    // how many of them are extra. None (saying why) when it can't start.
    fn sprite_sheet_job(&mut self) -> Option<(Vec<f32>, usize)> {
        if self.gpu_viewport.is_none() || self.flipbook_recording.is_some() {
            self.status = "Sprite sheets need the GPU viewport, with no flipbook or render running".into();
            return None;
        }
        let mut times = self.sprite_sheet_times();
        if times.len() > 1000 {
            self.status = format!("Sprite sheet: {} frames is too many; render at most 1000", times.len());
            return None;
        }
        // A seamless loop renders on past the end, to fade into the start.
        let s = &self.sprite_sheet;
        let blend = s.extra_frames(times.len());
        let step = s.every.max(1) as f32 / sims::SIM_FPS;
        let last = times.last().copied().unwrap_or(self.playback_start);
        times.extend((1..=blend).map(|i| last + i as f32 * step));
        Some((times, blend))
    }

    // Plays the sheet's frames, as they'll export, looping in the flipbook viewer.
    fn preview_sprite_sheet(&mut self) {
        let Some((times, blend)) = self.sprite_sheet_job() else { return };
        let s = &self.sprite_sheet;
        let capture = flipbook::Capture { render: Some([s.width.clamp(8, 4096), s.height.clamp(8, 4096)]), save_to: None, sheet: None, preview: Some(blend) };
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(capture)));
    }

    // Live re-exports keep the files current without adding to the Render Viewer.
    fn start_sprite_sheet(&mut self, path: PathBuf, live: bool) {
        let Some((times, blend)) = self.sprite_sheet_job() else { return };
        let s = &self.sprite_sheet;
        if s.live && !live {
            let exported = self.live_sheet_fingerprint();
            self.live_sheet = Some(LiveSheet { path: path.clone(), exported, pending: None });
        }
        let s = &self.sprite_sheet;
        let output = spritesheet::Output { path, columns: s.columns, trim: s.trim, fps: sims::SIM_FPS / s.every.max(1) as f32, live, blend, godot: s.godot, unity: s.unity, fill_grid: s.fill_grid, motion_vectors: s.motion_vectors, normal_map: s.normal_map, glow: s.glow };
        let capture = flipbook::Capture { render: Some([s.width.clamp(8, 4096), s.height.clamp(8, 4096)]), save_to: None, sheet: Some(output), preview: None };
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(capture)));
    }

    // Everything a sprite sheet depends on: the scene, the camera, the range and the sheet settings.
    fn live_sheet_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        scene_fingerprint(&self.graph.serialize()).hash(&mut hasher);
        let s = &self.sprite_sheet;
        format!(
            "{:?} {:?} {} {} {} {} {} {} {} {} {} {} {}",
            self.render_camera, self.cam_target, self.cam_rot_x, self.cam_rot_y, self.cam_zoom, self.cam_pan_x, self.cam_pan_y,
            self.playback_start, self.playback_end, s.width, s.height, s.every, s.columns
        )
        .hash(&mut hasher);
        (s.trim, s.seamless, s.blend, s.godot, s.unity, s.fill_grid, s.motion_vectors, s.normal_map, s.glow).hash(&mut hasher);
        hasher.finish()
    }

    // Exports a live sheet again once a change has settled. A change during a live export
    // stops it, and the next one starts once things are still again. Waits while the
    // timeline plays or a button is held, so it never fights with what you're doing.
    fn live_sheet_step(&mut self, ctx: &egui::Context) {
        if !self.sprite_sheet.live {
            if self.live_sheet.take().is_some() {
                self.status = "Live sprite sheet stopped".into();
            }
            return;
        }
        if self.live_sheet.is_none() {
            return;
        }
        let now = self.live_sheet_fingerprint();
        let exporting_live = self.flipbook_recording.as_ref().and_then(|r| r.capture.as_ref()).and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.live);
        let Some(live) = self.live_sheet.as_mut() else { return };
        if exporting_live && now != live.exported {
            if let Some(stopped) = self.flipbook_recording.take() {
                self.time = stopped.resume_time;
                self.dirty = true;
            }
        } else if self.flipbook_recording.is_some() {
            return;
        }
        if now == live.exported {
            live.pending = None;
            return;
        }
        match live.pending {
            Some((seen, since)) if seen == now => {
                let busy = self.animate || ctx.input(|i| i.pointer.any_down());
                if since.elapsed() >= LIVE_SHEET_SETTLE && !busy {
                    live.exported = now;
                    live.pending = None;
                    let path = live.path.clone();
                    self.start_sprite_sheet(path, true);
                }
            }
            _ => live.pending = Some((now, Instant::now())),
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    // Keeps the render options' camera in step with the toolbar's, as MoonRay renders do.
    fn render_camera_in_use(&mut self) {
        self.render_opts = RenderOpts { camera: self.render_camera, ..self.moonray_default_opts.clone() };
    }

    // Once this frame has cooked (simulations too), asks for it to be captured: a
    // window screenshot, or the GPU viewport's own render as it draws.
    fn flipbook_request_capture(&mut self, ctx: &egui::Context) {
        if let Some(recording) = self.flipbook_recording.as_mut() {
            if !recording.waiting && self.simulation_rx.is_none() {
                if recording.capture.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                }
                recording.waiting = true;
            }
            ctx.request_repaint();
        }
    }

    // Takes each frame's screenshot as it arrives and moves on; Escape cancels.
    fn flipbook_step(&mut self, ctx: &egui::Context) {
        let Some(recording) = self.flipbook_recording.as_mut() else { return };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Escape during a live export means stop keeping it up to date.
            if recording.capture.as_ref().and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.live) {
                self.sprite_sheet.live = false;
            }
            self.time = recording.resume_time;
            self.flipbook_recording = None;
            self.dirty = true;
            self.status = "Flipbook cancelled".into();
            return;
        }
        // The GPU viewport renders the frame as it draws (see viewport).
        if recording.waiting && recording.capture.is_some() {
            return;
        }
        if recording.waiting {
            let shot = ctx.input(|i| i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            }));
            let Some(shot) = shot else { return };
            if let Some(frame) = self.viewport_rect.and_then(|rect| flipbook::crop(&shot, rect, ctx.pixels_per_point())) {
                recording.frames.push(frame);
            }
            recording.next += 1;
            recording.waiting = false;
        }
        if recording.next >= recording.times.len() {
            let Some(done) = self.flipbook_recording.take() else { return };
            self.time = done.resume_time;
            self.dirty = true;
            let cut_off = || {
                let edges = spritesheet::edges_touched(&done.frames);
                if edges.is_empty() {
                    String::new()
                } else {
                    format!(". The effect runs off the {} of the frame and is cut off there: zoom the render camera out, or make the frame bigger", edges.join(" and "))
                }
            };
            if done.frames.is_empty() {
                self.status = "Flipbook: nothing was captured".into();
            } else if let Some(blend) = done.capture.as_ref().and_then(|c| c.preview) {
                let cut_off = cut_off();
                let frames = if blend > 0 { spritesheet::seamless(&done.frames, blend) } else { done.frames.clone() };
                let times = done.times[..frames.len().min(done.times.len())].to_vec();
                let looped = if blend > 0 { ", with the seamless blend" } else { "" };
                self.status = format!("Previewing the sprite sheet: {} frames as they'll export{looped}{cut_off}", frames.len());
                self.flipbook_viewer = Some(flipbook::Viewer::new(frames, times, sims::SIM_FPS / self.sprite_sheet.every.max(1) as f32));
            } else if let Some(output) = done.capture.as_ref().and_then(|c| c.sheet.clone()) {
                let cut_off = cut_off();
                let glows = (output.glow && done.glows.len() == done.frames.len()).then_some(&done.glows[..]);
                self.status = match spritesheet::save_with_glow(&done.frames, glows, &output) {
                    Ok((png, _)) if output.live => format!("Live sprite sheet: updated {}{cut_off}", png.display()),
                    Ok((png, sheet)) => {
                        // With Live on, the game shows the effect: don't open a window over it.
                        self.viewer.add(png.clone(), !self.sprite_sheet.live);
                        let (w, h) = sheet.image.dimensions();
                        format!("Sprite sheet: {} frames in {}×{} cells of {}×{} ({w}×{h} pixels), saved to {}{cut_off}", done.frames.len().saturating_sub(output.blend), sheet.columns, sheet.rows, sheet.cell[0], sheet.cell[1], png.display())
                    }
                    Err(e) => format!("Sprite sheet: {e}"),
                };
            } else if let Some(dir) = done.capture.and_then(|c| c.save_to) {
                self.status = match flipbook::save_pngs(&done.frames, &dir, "render") {
                    Ok(first) => {
                        self.viewer.add(first, !(done.frames.len() > 1));
                        let count = done.frames.len();
                        // A range plays as an animation in the flipbook viewer, at its own rate.
                        if count > 1 {
                            let step = (done.times[1] - done.times[0]).abs().max(1e-3);
                            self.flipbook_viewer = Some(flipbook::Viewer::new(done.frames, done.times, (1.0 / step).clamp(1.0, 120.0)));
                        }
                        format!("Rendered {count} frame{} in real time to {}{}", if count == 1 { "" } else { "s" }, dir.display(), if count > 1 { "; playing them in the flipbook viewer" } else { "" })
                    }
                    Err(e) => format!("Real-time render: couldn't save the frames: {e}"),
                };
            } else {
                self.status = format!("Flipbook: {} frames", done.frames.len());
                self.flipbook_viewer = Some(flipbook::Viewer::new(done.frames, done.times, sims::SIM_FPS));
            }
            return;
        }
        let t = recording.times[recording.next];
        if self.time != t {
            self.time = t;
            self.dirty = true;
        }
        let what = match recording.capture.as_ref() {
            Some(c) if c.sheet.as_ref().is_some_and(|o| o.live) => "Live sprite sheet",
            Some(c) if c.sheet.is_some() => "Sprite sheet",
            Some(c) if c.save_to.is_some() => "Real-time render",
            _ => "Flipbook",
        };
        self.status = format!("{what}: frame {} of {} (Esc cancels)", recording.next + 1, recording.times.len());
    }

    // The scene now matches what's on disk.
    fn mark_saved(&mut self) {
        self.saved_fingerprint = scene_fingerprint(&self.scene_text());
        self.unsaved = false;
        self.unsaved_checked = Some(Instant::now());
    }

    fn has_unsaved_changes(&self) -> bool {
        scene_fingerprint(&self.scene_text()) != self.saved_fingerprint
    }

    // What a saved scene file holds: the graph, then the timeline.
    fn scene_text(&self) -> String {
        self.graph.serialize() + &timeline_line(self.playback_start, self.playback_end, self.playback_fps)
    }

    // Uses a scene file's timeline, if it saved one.
    fn use_saved_timeline(&mut self, text: &str) {
        if let Some((start, end, fps)) = parse_timeline(text) {
            (self.playback_start, self.playback_end, self.playback_fps) = (start, end, fps);
            self.time = self.time.clamp(start, end);
        }
    }

    // Writes unsaved changes to the session's autosave every so often.
    fn autosave(&mut self, ctx: &egui::Context) {
        let Some(mut session) = self.recovery.take() else { return };
        if session.due() {
            let unsaved = self.has_unsaved_changes();
            let scene_path = self.scene_path.clone();
            session.tick(unsaved, scene_path.as_deref(), || self.scene_text());
        }
        self.recovery = Some(session);
        // An idle window doesn't redraw: wake it for the next autosave.
        ctx.request_repaint_after(recovery::AUTOSAVE_EVERY);
    }

    // After a crash or a forced quit: offers back the unsaved work, and the crash report.
    fn recovery_prompt(&mut self, ctx: &egui::Context) {
        let Some(offer) = self.recovery_offer.clone() else { return };
        let Some(dir) = recovery_dir() else { return };
        let name = offer.scene_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "an untitled scene".to_owned(), |n| format!("\"{}\"", n.to_string_lossy()));
        let when = offer.when.map(|t| recovery::ago(t, std::time::SystemTime::now()));
        let mut choice = None;
        egui::Modal::new(egui::Id::new("recovery_prompt")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("ProceduralFX Studio didn't close properly last time");
            ui.add_space(4.0);
            if offer.autosave.is_some() {
                ui.label(format!("Unsaved changes to {name} were kept{}.", when.map(|w| format!(" (autosaved {w})")).unwrap_or_default()));
            }
            if let Some(report) = &offer.crash_report {
                ui.label("It crashed. A crash report was saved on this computer; nothing has been sent.");
                ui.horizontal_wrapped(|ui| {
                    ui.weak("To help fix it, email the report to");
                    ui.hyperlink_to("support@build3dstudio.com", "mailto:support@build3dstudio.com?subject=Crash%20report");
                });
                ui.horizontal(|ui| {
                    if ui.button("Copy the report").clicked() {
                        if let Ok(text) = std::fs::read_to_string(report) {
                            ui.ctx().copy_text(text);
                            self.status = "Crash report copied".into();
                        }
                    }
                    if ui.button("Show the file").clicked() {
                        let _ = platform::reveal(report);
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if offer.autosave.is_some() {
                    if ui.button("Discard").on_hover_text("Delete the unsaved changes").clicked() {
                        choice = Some(false);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Recover").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            choice = Some(true);
                        }
                    });
                } else if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        let Some(recover) = choice else { return };
        if recover {
            if let Some(text) = offer.autosave.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                self.recover_scene(text, offer.scene_path.clone());
            }
        }
        recovery::forget(&dir, &offer.id);
        self.recovery_offer = None;
    }

    // Opens recovered work as unsaved changes to the scene it came from.
    fn recover_scene(&mut self, text: String, scene_path: Option<PathBuf>) {
        if let Some(problem) = scene_problem(&text) {
            self.status = format!("Couldn't recover the scene: {problem}");
            return;
        }
        self.undo.push(self.last_state.clone());
        self.use_saved_timeline(&text);
        self.load_state(text);
        // Unsaved against the file on disk (or against nothing, if it was untitled).
        let on_disk = scene_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        self.saved_fingerprint = on_disk.map_or(0, |t| scene_fingerprint(&t));
        self.unsaved = true;
        self.unsaved_checked = None;
        // Saving writes back to the scene it came from.
        self.scene_path = scene_path.filter(|p| p.extension().is_some_and(|e| e == SCENE_EXTENSION || e == "mhip"));
        self.status = "Recovered your unsaved changes. Save to keep them.".into();
    }

    // Quitting with unsaved changes asks first: Save, Don't Save or Cancel.
    fn confirm_quit(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.has_unsaved_changes() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.quit_prompt = true;
            }
        }
        if !self.quit_prompt {
            return;
        }
        let name = self.scene_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "this scene".to_owned(), |n| format!("\"{}\"", n.to_string_lossy()));
        let mut choice = None;
        egui::Modal::new(egui::Id::new("quit_prompt")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading("Save changes before quitting?");
            ui.label(format!("Your changes to {name} will be lost if you don't save them."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Don't Save").clicked() {
                    choice = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        choice = Some(true);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.quit_prompt = false;
                    }
                });
            });
        });
        match choice {
            Some(true) => {
                self.save_scene();
                // Quit only once it's saved (the Save dialog can be cancelled).
                if !self.has_unsaved_changes() {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                self.quit_prompt = false;
            }
            Some(false) => {
                self.quitting = true;
                self.quit_prompt = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        if self.unsaved_checked.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
            self.unsaved = self.has_unsaved_changes();
            self.unsaved_checked = Some(Instant::now());
        }
        let edited = if self.unsaved { " •" } else { "" };
        let title = match self.scene_path.as_ref().and_then(|p| p.file_name()) {
            Some(name) => format!("{}{edited} — ProceduralFX Studio", name.to_string_lossy()),
            None => format!("Untitled{edited} — ProceduralFX Studio"),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_moonray_settings {
            return;
        }
        let cameras = self.camera_ids();
        let node_selected = self
            .selected
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MoonRayRender { .. })));
        let rendering = self.moonray_rx.is_some() || self.sequence.is_some();
        // Live log while a render runs; the finished log afterwards.
        let log = match (&self.render_log_path, self.moonray_rx.is_some()) {
            (Some(path), true) => read_log_tail(path, 64 * 1024),
            _ => self.last_render_log.clone(),
        };
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        own_window(ctx, "render_settings", "Render Settings", [480.0, 720.0], [360.0, 320.0], &mut open, true, |ui| {
                if node_selected {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "A MoonRay render node is selected: Render uses that node's settings. Resolution scale still applies.",
                    );
                }
                // As tall as the window leaves room for, above the buttons and the log.
                egui::ScrollArea::vertical().max_height((ui.available_height() - 200.0).max(240.0)).show(ui, |ui| {
                    egui::CollapsingHeader::new("Renderer").default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Executable");
                            ui.add(egui::TextEdit::singleline(&mut self.moonray_default_executable).desired_width(260.0));
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Browse…").clicked() {
                                if let Some(path) = FileDialog::new().pick_file() {
                                    self.moonray_default_executable = path.to_string_lossy().into_owned();
                                }
                            }
                            if ui.button("Auto-detect").clicked() {
                                self.moonray_default_executable = detect_moonray_executable();
                            }
                            match resolve_executable(&self.moonray_default_executable) {
                                Some(path) => ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✔ {}", path.display())),
                                None => ui.colored_label(egui::Color32::from_rgb(240, 110, 110), "✖ not found"),
                            };
                        });
                    });

                    egui::CollapsingHeader::new("Image").default_open(true).show(ui, |ui| {
                        let (w, h) = (&mut self.moonray_default_width, &mut self.moonray_default_height);
                        let current = RESOLUTION_PRESETS
                            .iter()
                            .find(|(_, pw, ph)| *pw == *w && *ph == *h)
                            .map(|(name, ..)| *name)
                            .unwrap_or("Custom");
                        ui.horizontal(|ui| {
                            ui.label("Resolution");
                            egui::ComboBox::from_id_salt("render_resolution_preset")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    for (name, pw, ph) in RESOLUTION_PRESETS {
                                        if ui.selectable_label(current == name, format!("{name}  ({pw}×{ph})")).clicked() {
                                            (*w, *h) = (pw, ph);
                                        }
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            pfield(ui, pdrag(w).range(1..=16384).prefix("W "));
                            if ui.button("⇄").on_hover_text("Swap width and height").clicked() {
                                std::mem::swap(w, h);
                            }
                            pfield(ui, pdrag(h).range(1..=16384).prefix("H "));
                            ui.weak(format!("aspect {:.3}", *w as f32 / (*h).max(1) as f32));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Resolution scale");
                            for (scale, label) in [(1.0, "100%"), (0.5, "50%"), (0.25, "25%")] {
                                ui.selectable_value(&mut self.resolution_scale, scale, label);
                            }
                        })
                        .response
                        .on_hover_text("Quick previews: scales every render, render nodes included");
                        if self.resolution_scale < 1.0 {
                            ui.weak(format!(
                                "Renders at {}×{}",
                                ((*w as f32 * self.resolution_scale).round() as usize).max(1),
                                ((*h as f32 * self.resolution_scale).round() as usize).max(1)
                            ));
                        }
                        ui.horizontal(|ui| {
                            ui.label("Camera");
                            ui.push_id("settings_camera", |ui| render_camera_combo(ui, &mut self.render_camera, &cameras));
                        });
                        ui.checkbox(&mut self.moonray_default_opts.stage, "Render the STAGE (USD) instead of the OBJ objects")
                            .on_hover_text("Uses the displayed LOP's stage: its meshes, materials, lights and (with the Auto camera) its first camera");
                        hydra_checkbox(ui, &mut self.moonray_default_opts);
                    });

                    egui::CollapsingHeader::new("Sampling").default_open(true).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        let samples = &mut self.moonray_default_samples;
                        ui.horizontal(|ui| {
                            ui.label("Quality");
                            for (name, spp, depth, light, bsdf) in QUALITY_PRESETS {
                                let active = *samples == spp && opts.max_depth == depth && opts.light_samples == light && opts.bsdf_samples == bsdf;
                                if ui
                                    .selectable_label(active, name)
                                    .on_hover_text(format!("{spp} spp · depth {depth} · light {light} · BSDF {bsdf}"))
                                    .clicked()
                                {
                                    *samples = spp;
                                    (opts.max_depth, opts.light_samples, opts.bsdf_samples) = (depth, light, bsdf);
                                }
                            }
                        });
                        pfield(ui, pdrag(samples).range(1..=4096).prefix("Samples per pixel "));
                        pfield(ui, pslider(&mut opts.max_depth, 1..=32).text("Max ray depth"));
                        pfield(ui, pslider(&mut opts.light_samples, 1..=16).text("Light samples"));
                        pfield(ui, pslider(&mut opts.bsdf_samples, 1..=16).text("BSDF samples"));
                        pfield(ui, pslider(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                            .on_hover_text("Light samples per step inside smoke and fire. Render time grows roughly in proportion: 1 for previews, 4+ for clean finals.");
                        ui.horizontal(|ui| {
                            ui.label("Environment light");
                            ui.color_edit_button_rgb(&mut opts.background);
                        });
                        ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                    });

                    egui::CollapsingHeader::new("Output").default_open(true).show(ui, |ui| {
                        ui.radio_value(&mut self.render_to_temp, true, "Temporary folder (auto-named, open in the viewer)");
                        if self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label("Format");
                                ui.selectable_value(&mut self.render_temp_ext, "exr", "EXR (HDR)");
                                ui.selectable_value(&mut self.render_temp_ext, "png", "PNG");
                                if ui.button("Open folder").clicked() {
                                    if let Err(error) = reveal_in_file_browser(&std::env::temp_dir()) {
                                        self.status = error;
                                    }
                                }
                            });
                        }
                        ui.radio_value(&mut self.render_to_temp, false, "Save to file");
                        if !self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.add(egui::TextEdit::singleline(&mut self.moonray_default_output).desired_width(260.0));
                                if ui.button("Choose…").clicked() {
                                    if let Some(path) = FileDialog::new()
                                        .set_file_name("render.exr")
                                        .add_filter("Images", &["exr", "png", "jpg", "tif"])
                                        .save_file()
                                    {
                                        self.moonray_default_output = path.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            if PathBuf::from(&self.moonray_default_output).is_relative() {
                                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "Relative paths are written to the temp folder; choose a full path.");
                            }
                        }
                        ui.checkbox(&mut self.open_viewer_after_render, "Open in the Render Viewer when finished");
                        ui.checkbox(&mut self.bucket_render, "Progressive buckets")
                            .on_hover_text("Shows the image filling in tile by tile in the Render Viewer. Each tile restarts MoonRay, so turn this off for very heavy scenes.");
                    });

                    egui::CollapsingHeader::new("Frame range").default_open(false).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        ui.checkbox(&mut opts.sequence, "Render frame range");
                        if opts.sequence {
                            ui.horizontal(|ui| {
                                pfield(ui, pdrag(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start ").suffix(" s"));
                                pfield(ui, pdrag(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End ").suffix(" s"));
                                pfield(ui, pdrag(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step ").suffix(" s"))
                                    .on_hover_text(format!("Seconds between rendered frames: {:.4} is every frame", 1.0 / sims::SIM_FPS));
                            });
                            if ui.button("Use timeline range").clicked() {
                                opts.range_start = self.playback_start.max(0.0);
                                opts.range_end = self.playback_end.max(opts.range_start);
                            }
                            let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                            ui.small(format!("{frames} frames, numbered render.0001, render.0002, … (#### in a file name sets the position)."));
                        }
                    });

                    egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
                        ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                        ui.text_edit_singleline(&mut self.moonray_default_arguments);
                        ui.label("Extra MoonRay arguments");
                        ui.add(egui::TextEdit::singleline(&mut self.render_extra_args).hint_text("e.g. -threads 8"));
                        if ui.button("Reset all settings to defaults").clicked() {
                            let defaults = Self::default();
                            self.moonray_default_opts = RenderOpts::default();
                            self.moonray_default_arguments = defaults.moonray_default_arguments;
                            self.moonray_default_width = defaults.moonray_default_width;
                            self.moonray_default_height = defaults.moonray_default_height;
                            self.moonray_default_samples = defaults.moonray_default_samples;
                            self.render_to_temp = defaults.render_to_temp;
                            self.render_temp_ext = defaults.render_temp_ext;
                            self.resolution_scale = defaults.resolution_scale;
                            self.open_viewer_after_render = defaults.open_viewer_after_render;
                            self.render_extra_args.clear();
                        }
                    });

                    egui::CollapsingHeader::new("Render log").default_open(false).show(ui, |ui| {
                        if !self.last_render_summary.is_empty() {
                            ui.label(&self.last_render_summary);
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!log.is_empty(), egui::Button::new("Copy log")).clicked() {
                                ui.ctx().copy_text(log.clone());
                            }
                            if let Some(path) = &self.render_log_path {
                                if ui.button("Reveal log file").clicked() {
                                    if let Err(error) = reveal_in_file_browser(path) {
                                        self.status = error;
                                    }
                                }
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("render_log").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                            let text = if log.is_empty() { "No render yet." } else { log.as_str() };
                            ui.add(egui::Label::new(egui::RichText::new(text).monospace().small()).wrap());
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if rendering {
                        ui.spinner();
                        if let Some(started) = self.render_started {
                            ui.label(format!("Rendering… {:.0}s", started.elapsed().as_secs_f32()));
                        }
                        cancel = ui.button("■ Cancel").clicked();
                    } else {
                        let label = if self.moonray_default_opts.sequence && !node_selected { "Render sequence" } else { "Render" };
                        start = ui.button(label).clicked();
                    }
                    if ui.button("🖼 Render Viewer").clicked() {
                        self.viewer.show();
                    }
                });
            });
        self.show_moonray_settings = open;
        if start {
            self.start_moonray_render();
        }
        if cancel {
            self.cancel_render();
        }
    }

    // Stops the running render (killing MoonRay) and any sequence it belongs to.
    fn cancel_render(&mut self) {
        if let Some(cancel) = &self.render_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.sequence = None;
        self.render_at = None;
        self.status = "Cancelling render...".into();
    }

    fn spawn_moonray_render(&mut self, executable: String, arguments: String, output: String, width: usize, height: usize, samples: usize) {
        // Lights and cameras are read from the nodes: evaluate them at the frame being rendered.
        let render_time = self.render_at.unwrap_or(self.time);
        self.graph.apply_animation(render_time);
        // Those values are not user edits for Auto Key.
        self.anim_snapshot = None;
        if executable.trim().is_empty() || output.trim().is_empty() {
            self.status = "MoonRay executable and output image are required".into();
            return;
        }
        let scale = self.resolution_scale.clamp(0.05, 1.0);
        let width = ((width as f32 * scale).round() as usize).max(1);
        let height = ((height as f32 * scale).round() as usize).max(1);

        // Preserve the requested output path and export the scene to a writable
        // temporary file to avoid failures when the configured output path is
        // on a read-only filesystem (for example when running from inside a
        // .app bundle Resources directory).
        let output_path = PathBuf::from(&output);
        
        // Export the scene to temp
        // (use a unique filename to avoid clobbering concurrent renders)
        // Export the scene to a writable temporary file to avoid failures when
        // the configured output path is on a read-only filesystem (for example
        // when running from inside a .app bundle Resources directory).
        let tmp = std::env::temp_dir();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        // Through Hydra: hd_render reads the stage as USD, and renders through its first camera.
        let hydra = self.render_opts.stage && self.render_opts.hydra;
        let mut hydra_camera = None;
        let scene_path = if hydra {
            let usda = tmp.join(format!("proceduralfx_stage_{ts}.usda"));
            match self.write_hydra_scene(&usda) {
                Ok(camera) => hydra_camera = camera,
                Err(error) => {
                    self.status = error;
                    return;
                }
            }
            usda
        } else {
        let scene_filename = format!("proceduralfx_scene_{}.obj", ts);
        let scene_path = tmp.join(scene_filename);
        let scene = match self.render_scene() {
            Ok(scene) => scene,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let scene_parts = &scene.parts;
        let scene_mesh = scene_parts
            .iter()
            .fold(Mesh::default(), |merged, (mesh, _)| node_merge(merged, mesh.clone()));
        if let Err(error) = scene_mesh.export_obj(scene_path.to_string_lossy().as_ref()) {
            self.status = format!("MoonRay scene export failed: {error}");
            return;
        }

        // Also attempt to generate a minimal RDLA wrapper that embeds the
        // exported mesh as vertex/index arrays. MoonRay accepts .rdla files
        // via `-in`, so prefer that when available. This is a best-effort
        // ASCII RDLA writer — the format may need adjustments depending on
        // the MoonRay build; we iterate as needed.
        let rdla_filename = format!("proceduralfx_scene_{}.rdla", ts);
        let rdla_path = tmp.join(rdla_filename);
        let rdla_result = self.export_scene_rdla(&rdla_path, &scene, width, height, samples);
        // If RDLA export succeeded, use it as the scene input; otherwise
        // fall back to the OBJ we wrote above.
        if rdla_result.is_ok() { rdla_path } else { scene_path }
        };

        let replace = |text: &str| {
            text.replace("{scene}", &scene_path.to_string_lossy())
                .replace("{width}", &width.to_string())
                .replace("{height}", &height.to_string())
                .replace("{samples}", &samples.to_string())
        };
        // Resolve executable: if the configured executable doesn't exist or
        // is relative, try to find a bundled MoonRay inside the app bundle
        // (Contents/Resources/moonray or Contents/MacOS/moonray).
        let mut executable_to_run = executable.clone();
        let mut exe_path = PathBuf::from(&executable_to_run);
        if !exe_path.is_absolute() || !exe_path.exists() {
            if let Ok(cur) = std::env::current_exe() {
                if let Some(bundle_dir) = cur.parent().and_then(|p| p.parent()) {
                    let candidates = [
                        bundle_dir.join("Resources").join("moonray"),
                        bundle_dir.join("MacOS").join("moonray"),
                        bundle_dir.join("bin").join("moonray"),
                    ];
                    for c in &candidates {
                        if c.exists() {
                            exe_path = c.clone();
                            break;
                        }
                    }
                }
            }
        }

        // Windows: the helper runs MoonRay or hd_render in a container, and the scene's
        // own file paths (textures, HDRs) are rewritten to the container's view.
        let wsl = wslrender::is_helper(&exe_path.to_string_lossy());
        if wsl {
            if let Err(error) = wslrender::prepare_scene(&scene_path) {
                self.status = error;
                return;
            }
        }
        if hydra && !wsl {
            match find_hd_render(&exe_path) {
                Some(path) => exe_path = path,
                None => {
                    self.status = "Render through Hydra: hd_render was not found (next to MoonRay, in the app, or on PATH)".into();
                    return;
                }
            }
        }

        // Ensure execute permission on Unix-like systems if the file exists.
        if exe_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&exe_path) {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o111 == 0 {
                        perms.set_mode(mode | 0o111);
                        let _ = std::fs::set_permissions(&exe_path, perms);
                    }
                }
            }
            executable_to_run = exe_path.to_string_lossy().into_owned();
        }

        // Decide where MoonRay should write its output. If the requested
        // output path is not writable (for example inside the app bundle),
        // write to a temp file instead and copy the result back after render.
        let orig_output_pb = PathBuf::from(&output);
        let mut used_output_pb = orig_output_pb.clone();
        let mut copy_back_target = orig_output_pb.clone();
        let tmp = std::env::temp_dir();
        // Relative output paths are ambiguous in packaged apps (Finder launch
        // may set an unwritable working directory). Route them to temp.
        if orig_output_pb.is_relative() {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if self.render_at.is_some() {
                // Sequence frames keep their numbered names in one folder.
                let folder = tmp.join("proceduralfx_sequence");
                let _ = std::fs::create_dir_all(&folder);
                if let Some(name) = orig_output_pb.file_name() {
                    used_output_pb = folder.join(name);
                }
            }
            // No copy-back for relative paths; report the temp output path.
            copy_back_target = used_output_pb.clone();
        }
        // try to ensure parent exists and try creating the file to test writability
        let output_writable = (|| {
            if let Some(parent) = used_output_pb.parent() {
                // For relative paths with no explicit parent (e.g.
                // "procedural_output.png"), parent is empty; do not assume
                // writability — we still probe by attempting to create the
                // file below.
                if !parent.as_os_str().is_empty() {
                    if let Err(_) = std::fs::create_dir_all(parent) {
                        return false;
                    }
                }
            }
            match std::fs::OpenOptions::new().write(true).create(true).open(&used_output_pb) {
                Ok(mut f) => {
                    let _ = f.flush();
                    let _ = std::fs::remove_file(&used_output_pb);
                    true
                }
                Err(_) => false,
            }
        })();
        if !output_writable {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if copy_back_target.is_relative() {
                copy_back_target = used_output_pb.clone();
            }
        }

        // Now perform argument replacement with the (possibly) adjusted output path
        let arguments = replace(&arguments).replace("{output}", &used_output_pb.to_string_lossy());
        let output_path = used_output_pb.to_string_lossy().into_owned();
        self.pending_render_output = Some(copy_back_target.to_string_lossy().into_owned());
        let (tx, rx) = mpsc::channel();
        self.moonray_rx = Some(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.render_cancel = Some(cancel.clone());
        let log_path = tmp.join(format!("proceduralfx_render_{ts}.log"));
        self.render_log_path = Some(log_path.clone());
        self.render_started = Some(Instant::now());
        let extra_args = self.render_extra_args.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        self.status = format!("Rendering with {}: {executable_to_run}", if hydra { "Hydra (hdMoonray)" } else { "MoonRay" });
        // The Render Viewer opens on every render; bucket mode streams the image into it.
        // hd_render only writes the finished image.
        // (Not through the Windows helper: every bucket would start a container.)
        let progress = (self.bucket_render && !hydra && !wsl).then(|| Arc::new(std::sync::Mutex::new(render_viewer::RenderProgress::new(width, height))));
        match &progress {
            Some(progress) => self.viewer.watch(progress.clone()),
            None => self.viewer.show(),
        }
        let exec_run = executable_to_run.clone();
        let copy_back_target_clone = copy_back_target.clone();
        let requested_output = output.clone();
        let scene_for_hydra = scene_path.to_string_lossy().into_owned();
        let hydra_env = hydra.then(|| hydra_plugin_path(&PathBuf::from(&exec_run))).flatten();
        std::thread::spawn(move || {
                // hd_render takes the stage and output directly; MoonRay's argument template doesn't apply.
                let raw_tokens = if hydra {
                    // hdMoonray's in-process renderer: the default one renders through Arras, which
                    // runs MoonRay as a separate compute process with its own session setup.
                    let mut tokens = vec!["-renderer".to_string(), "HdMoonrayRendererDebugPlugin".into(), "-in".into(), scene_for_hydra.clone(), "-out".into(), output_path.clone(), "-size".into(), width.to_string(), height.to_string()];
                    if let Some(camera) = &hydra_camera {
                        tokens.extend(["-camera".to_string(), camera.clone()]);
                    }
                    tokens
                } else {
                    arguments.split_whitespace().map(str::to_owned).collect::<Vec<_>>()
                };
                // Map common templates/flags to MoonRay CLI (-in, -out, -size).
                let mut args = Vec::new();
                let mut i = 0usize;
                while i < raw_tokens.len() {
                    let t = &raw_tokens[i];
                    if t.starts_with("--scene=") {
                        args.push("-in".to_string());
                        args.push(t[8..].to_string());
                    } else if t == "--scene" {
                        args.push("-in".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--output=") {
                        args.push("-out".to_string());
                        args.push(t[9..].to_string());
                    } else if t == "--output" {
                        args.push("-out".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--size=") {
                        // --size=WxH or --size W H; try to split by x or comma
                        let val = t[7..].to_string();
                        let mut parts: Vec<&str> = val.split(|c| c == 'x' || c == 'X' || c == ',').collect();
                        if parts.len() == 1 && i + 2 < raw_tokens.len() {
                            parts = vec![&raw_tokens[i+1], &raw_tokens[i+2]];
                            i += 2;
                        }
                        args.push("-size".to_string());
                        for p in parts { args.push(p.to_string()); }
                    } else if t == "--size" {
                        args.push("-size".to_string());
                        if i + 2 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); i += 1; args.push(raw_tokens[i].clone()); }
                    } else {
                        args.push(t.clone());
                    }
                    i += 1;
                }

                // If the user-provided template produced bare positional
                // tokens (e.g. "{scene} {output} {width} {height} {samples}"),
                // insert MoonRay flags where missing so MoonRay sees `-in` and
                // `-out`. Also ensure a `-size` is present.
                if !args.iter().any(|a| a == "-in") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".rdla") || t.ends_with(".rdlb") || t.ends_with(".obj") || t.ends_with(".ply") || t.ends_with(".usd") {
                            args.insert(idx, "-in".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-out") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".png") || t.ends_with(".exr") || t.ends_with(".jpg") || t.ends_with(".jpeg") || t.ends_with(".tif") || t.ends_with(".tiff") {
                            args.insert(idx, "-out".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-size") {
                    args.push("-size".to_string());
                    args.push(width.to_string());
                    args.push(height.to_string());
                }

                // Final output-path normalization: regardless of what the
                // user template provided, make MoonRay write to our vetted
                // writable path.
                let mut out_flag_found = false;
                let mut idx = 0usize;
                while idx < args.len() {
                    if args[idx] == "-out" {
                        out_flag_found = true;
                        if idx + 1 < args.len() {
                            args[idx + 1] = output_path.clone();
                        } else {
                            args.push(output_path.clone());
                        }
                        idx += 2;
                        continue;
                    }
                    if args[idx] == requested_output {
                        args[idx] = output_path.clone();
                    }
                    idx += 1;
                }
                if !out_flag_found {
                    args.push("-out".to_string());
                    args.push(output_path.clone());
                }

                // Extra flags are MoonRay's; hd_render rejects flags it doesn't know.
                if !hydra {
                    args.extend(extra_args);
                }
                let mut dso_paths: Option<String> = None;
                // If running from inside an app bundle, help MoonRay find its
                // RDL DSOs by setting RDL2_DSO_PATH to the bundle Resources
                // directory (where packaging should copy DSOs). Also include
                // the executable parent as a fallback.
                if let Ok(cur_exe) = std::env::current_exe() {
                    if let Some(bundle_dir) = cur_exe.parent().and_then(|p| p.parent()) {
                        let mut candidates = Vec::new();
                        candidates.push(bundle_dir.join("Resources").join("rdl2dso"));
                        candidates.push(bundle_dir.join("Resources"));
                        candidates.push(bundle_dir.join("MacOS"));
                        candidates.push(bundle_dir.join("rdl2dso"));
                        if let Ok(existing) = std::env::var("RDL2_DSO_PATH") {
                            for p in existing.split(':') {
                                if !p.is_empty() {
                                    candidates.push(std::path::PathBuf::from(p));
                                }
                            }
                        }
                        // join as colon-separated paths
                        let paths = candidates.iter()
                            .filter(|p| p.exists())
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(":");
                        if !paths.is_empty() {
                            dso_paths = Some(paths);
                        }
                    }
                }
                let make_cmd = |args: &[String]| {
                    let mut cmd = Command::new(&exec_run);
                    platform::no_window(&mut cmd);
                    if wsl {
                        // The container has MoonRay's own environment.
                        cmd.arg(if hydra { "hd_render" } else { "moonray" });
                        cmd.args(args);
                        return cmd;
                    }
                    cmd.args(args);
                    if let Some(paths) = &dso_paths {
                        cmd.env("RDL2_DSO_PATH", paths);
                    }
                    // hdMoonray's USD plugin: the bundled one is found next to USD itself.
                    if let Some(plugins) = &hydra_env {
                        let existing = std::env::var("PXR_PLUGINPATH_NAME").unwrap_or_default();
                        cmd.env("PXR_PLUGINPATH_NAME", if existing.is_empty() { plugins.clone() } else { format!("{plugins}:{existing}") });
                    }
                    cmd
                };
                let outcome: Result<(), String> = match &progress {
                    Some(progress) => render_buckets(&make_cmd, &args, &output_path, width, height, &log_path, &cancel, progress),
                    None => run_logged(&mut make_cmd(&args), &log_path, &cancel)
                        .map_err(|error| if cancel.load(Ordering::Relaxed) { error } else { format!("Could not start {exec_run}: {error}") })
                        .and_then(|status| {
                            if status.success() {
                                Ok(())
                            } else {
                                let tail = read_log_tail(&log_path, 600);
                                let hint = wslrender::failure_hint(&tail).map(|h| format!("{h}. ")).unwrap_or_default();
                                Err(format!("{hint}{} exited with {status}: {}", if hydra { "hd_render" } else { "MoonRay" }, tail.trim()))
                            }
                        }),
                };
                let result = outcome.and_then(|()| {
                    {
                        let rendered_pb = PathBuf::from(&output_path);
                        let _ = make_png_opaque(&rendered_pb);
                        let replaced_preview = ensure_png_not_all_black(&rendered_pb).unwrap_or(false);
                        // If we rendered to a temp file because the requested
                        // output path was not writable, copy it back to the
                        // original requested location.
                        if copy_back_target_clone != PathBuf::from(&output_path) {
                            let _ = std::fs::create_dir_all(copy_back_target_clone.parent().unwrap_or_else(|| std::path::Path::new(".")));
                            match std::fs::copy(&output_path, &copy_back_target_clone) {
                                Ok(_) => {
                                    let _ = make_png_opaque(&copy_back_target_clone);
                                    let _ = ensure_png_not_all_black(&copy_back_target_clone);
                                    if replaced_preview {
                                        return Ok((format!("MoonRay render complete: {} (copied to {}) [fallback preview inserted because output was fully black]", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                    }
                                    return Ok((format!("MoonRay render complete: {} (copied to {})", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                }
                                Err(e) => return Err(format!("Render complete but failed to copy output to {}: {}", copy_back_target_clone.display(), e)),
                            }
                        }
                        if replaced_preview {
                            return Ok((format!("MoonRay render complete: {output_path} [fallback preview inserted because output was fully black]"), rendered_pb));
                        }
                        Ok((format!("MoonRay render complete: {output_path}"), rendered_pb))
                    }
                });
            let log = read_log_tail(&log_path, 256 * 1024);
            let _ = tx.send(RenderOutcome { result, log });
        });
    }

    fn poll_moonray_render(&mut self) {
        let Some(rx) = self.moonray_rx.as_ref() else {
            return;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Disconnected) => RenderOutcome {
                result: Err("MoonRay render worker disconnected".to_string()),
                log: String::new(),
            },
            Err(mpsc::TryRecvError::Empty) => {
                // The Windows helper says what it's doing (downloading MoonRay the first time).
                if self.render_log_checked.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
                    self.render_log_checked = Some(Instant::now());
                    if let Some(step) = self.render_log_path.as_ref().map(|p| read_log_tail(p, 4096)).as_deref().and_then(wslrender::progress) {
                        self.status = step;
                    }
                }
                return;
            }
        };
        self.moonray_rx = None;
        self.render_cancel = None;
        let seconds = self.render_started.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        self.last_render_log = outcome.log;
        self.render_ok = Some(outcome.result.is_ok());
        if outcome.result.is_ok() {
            if let Some(path) = self.pending_render_output.take() {
                cops::set_last_render(&path);
                // Render COPs read it again.
                self.dirty = true;
            }
        }
        let outcome = outcome.result.map(|(message, image)| {
            self.viewer.add(image, self.open_viewer_after_render);
            format!("{message} in {seconds:.1}s")
        });
        self.last_render_summary = match &outcome {
            Ok(message) => message.clone(),
            Err(error) => format!("Failed after {seconds:.1}s: {error}"),
        };
        if self.sequence.is_none() {
            self.status = outcome.unwrap_or_else(|error| error);
            return;
        }
        // Within a sequence only failures surface; advance_sequence reports progress.
        if let Err(error) = outcome {
            self.sequence = None;
            self.render_at = None;
            self.status = format!("Sequence stopped: {error}");
        }
    }

    // Starts cooking a TOP node: its input chain's work items first, then its own.
    fn start_top_cook(&mut self, id: NodeId) {
        if self.top_run.is_some() {
            self.status = "A TOP network is already cooking".into();
            return;
        }
        // The chain from the first node down to `id`.
        let mut chain = vec![id];
        while let Some(src) = self.graph.input_of(*chain.last().unwrap(), 0) {
            if chain.contains(&src) || chain.len() > 64 {
                break;
            }
            chain.push(src);
        }
        chain.reverse();
        let mut items = HashMap::new();
        let mut queue = Vec::new();
        for node in &chain {
            match self.graph.top_items(*node) {
                Ok(list) => {
                    queue.extend((0..list.len()).map(|i| (*node, i)));
                    self.top_results.insert(*node, vec![(tops::State::Waiting, String::new()); list.len()]);
                    items.insert(*node, list);
                }
                Err(e) => {
                    self.status = format!("{}: {e}", self.graph.nodes.get(node).map_or(String::new(), |n| n.title(*node)));
                    return;
                }
            }
        }
        queue.reverse();
        let total = queue.len();
        self.top_run = Some(TopRun { queue, items, current: None, total, failed: 0 });
        self.status = format!("Cooking {total} work items…");
        self.advance_top_run();
    }

    fn set_item_state(&mut self, node: NodeId, index: usize, state: tops::State, log: String) {
        if let Some(slot) = self.top_results.get_mut(&node).and_then(|v| v.get_mut(index)) {
            *slot = (state, log);
        }
    }

    // Runs the TOP cook: finishes the running item when it's done, then starts the next.
    fn advance_top_run(&mut self) {
        if std::mem::take(&mut self.top_cancel_request) {
            if let Some(run) = self.top_run.take() {
                if let Some(cancel) = &self.render_cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                for (node, index) in run.queue {
                    self.set_item_state(node, index, tops::State::Failed, "cancelled".into());
                }
                if let Some(TopJob { node, index, .. }) = run.current {
                    self.set_item_state(node, index, tops::State::Failed, "cancelled".into());
                }
                self.status = "TOP cook cancelled".into();
            }
            return;
        }
        loop {
            let Some(run) = self.top_run.as_mut() else { return };
            // The running item.
            if let Some(job) = run.current.as_ref() {
                let finished = match &job.wait {
                    TopWait::Render => {
                        if self.moonray_rx.is_some() {
                            return;
                        }
                        let ok = self.render_ok.unwrap_or(false);
                        Some((ok, if ok { job.output.clone() } else { self.last_render_summary.clone() }))
                    }
                    TopWait::Shell(rx) => match rx.try_recv() {
                        Ok(result) => Some(match result {
                            Ok(log) => (true, log),
                            Err(log) => (false, log),
                        }),
                        Err(mpsc::TryRecvError::Empty) => return,
                        Err(mpsc::TryRecvError::Disconnected) => Some((false, "the command stopped unexpectedly".into())),
                    },
                };
                let (node, index) = (job.node, job.index);
                let run = self.top_run.as_mut().unwrap();
                run.current = None;
                if let Some((ok, log)) = finished {
                    if !ok {
                        run.failed += 1;
                    }
                    self.set_item_state(node, index, if ok { tops::State::Done } else { tops::State::Failed }, log);
                }
                continue;
            }
            let Some((node, index)) = run.queue.pop() else {
                let (total, failed) = (run.total, run.failed);
                self.top_run = None;
                self.render_at = None;
                self.status = if failed == 0 { format!("TOP cook finished: {total} work items") } else { format!("TOP cook finished: {failed} of {total} work items failed") };
                return;
            };
            let item = run.items[&node][index].clone();
            self.set_item_state(node, index, tops::State::Running, String::new());
            match item.job.clone() {
                tops::Job::Nothing => self.set_item_state(node, index, tops::State::Done, item.output.clone()),
                tops::Job::Export(target) => {
                    let result = self.with_wedges(&item, |app| {
                        let source = app.graph.input_of(target, 0).ok_or("the File Export node has no input")?;
                        let mesh = app.graph.cook_fresh(Some(source), item.time(sims::SIM_FPS));
                        write_geometry_file(&mesh, &item.output).map(|_| item.output.clone())
                    });
                    let run = self.top_run.as_mut().unwrap();
                    if result.is_err() {
                        run.failed += 1;
                    }
                    let (state, log) = match result {
                        Ok(out) => (tops::State::Done, out),
                        Err(e) => (tops::State::Failed, e),
                    };
                    self.set_item_state(node, index, state, log);
                }
                tops::Job::Cache(target) => {
                    let result = self.with_wedges(&item, |app| {
                        let source = app.graph.input_of(target, 0).ok_or("the Cache node has no input")?;
                        let mesh = app.graph.cook_fresh(Some(source), item.time(sims::SIM_FPS));
                        save_mesh_cache(std::path::Path::new(&item.output), &mesh).map(|_| item.output.clone()).map_err(|e| format!("couldn't write {}: {e}", item.output))
                    });
                    let run = self.top_run.as_mut().unwrap();
                    if result.is_err() {
                        run.failed += 1;
                    }
                    let (state, log) = match result {
                        Ok(out) => (tops::State::Done, out),
                        Err(e) => (tops::State::Failed, e),
                    };
                    self.set_item_state(node, index, state, log);
                }
                tops::Job::Render(target) => {
                    let Some(NodeKind::MoonRayRender { executable, arguments, width, height, samples, opts, .. }) = self.graph.nodes.get(&target).map(|n| n.kind.clone()) else {
                        self.set_item_state(node, index, tops::State::Failed, "the render node is gone".into());
                        continue;
                    };
                    if self.moonray_rx.is_some() {
                        // Another render is running: try again next frame.
                        let run = self.top_run.as_mut().unwrap();
                        run.queue.push((node, index));
                        return;
                    }
                    self.render_ok = None;
                    let output = item.output.clone();
                    self.with_wedges(&item, |app| {
                        app.render_opts = opts.clone();
                        app.render_at = Some(item.time(sims::SIM_FPS));
                        // Wedged parameters must not come from the cache of unchanging nodes.
                        let cache = std::mem::take(&mut app.static_cache);
                        app.spawn_moonray_render(executable.clone(), arguments.clone(), output.clone(), width, height, samples);
                        app.static_cache = cache;
                        Ok::<(), String>(())
                    })
                    .ok();
                    if self.moonray_rx.is_none() {
                        let reason = self.status.clone();
                        self.top_run.as_mut().unwrap().failed += 1;
                        self.set_item_state(node, index, tops::State::Failed, reason);
                        continue;
                    }
                    self.status = format!("TOP: rendering {output}");
                    self.top_run.as_mut().unwrap().current = Some(TopJob { node, index, output, wait: TopWait::Render });
                    return;
                }
                tops::Job::Shell(command) => {
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = platform::shell(&command).output();
                        let _ = tx.send(match result {
                            Ok(out) => {
                                let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_owned();
                                if out.status.success() { Ok(log) } else { Err(if log.is_empty() { format!("exit status {}", out.status) } else { log }) }
                            }
                            Err(e) => Err(e.to_string()),
                        });
                    });
                    self.top_run.as_mut().unwrap().current = Some(TopJob { node, index, output: item.output.clone(), wait: TopWait::Shell(rx) });
                    return;
                }
            }
        }
    }

    // Runs `f` with the item's wedged parameters set, putting them back afterwards.
    fn with_wedges<T>(&mut self, item: &tops::WorkItem, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        let mut restore = Vec::new();
        for (node, param, value) in &item.wedges {
            if let Some(old) = self.graph.set_param(*node, *param, *value) {
                restore.push((*node, *param, old));
            }
        }
        let result = f(self);
        for (node, param, old) in restore.into_iter().rev() {
            self.graph.set_param(node, param, old);
        }
        result
    }

    // File Output COP: writes the cooked image (one file per frame over the timeline when asked).
    fn write_cop_output(&mut self, id: NodeId, range: bool) -> Result<String, String> {
        let Some(NodeKind::Cop(cop)) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return Err("not a File Output node".into());
        };
        let file = cop.texts.first().map(|f| f.trim().to_owned()).filter(|f| !f.is_empty()).ok_or("choose a file to write")?;
        let times = if range { frame_times(self.playback_start.max(0.0), self.playback_end, 1.0 / sims::SIM_FPS) } else { vec![self.time] };
        let mut written = Vec::new();
        for time in times {
            let mesh = self.graph.cook_fresh(Some(id), time);
            let img = cops::Image::from_mesh(&mesh).ok_or_else(|| node_errors(|e| e.get(&id).cloned()).unwrap_or_else(|| "no image to write".into()))?;
            let path = fileio::expand_frame(&file, time, sims::SIM_FPS);
            cops::write_image(&img, &path)?;
            written.push(path);
        }
        Ok(match written.as_slice() {
            [one] => format!("Wrote {one}"),
            many => format!("Wrote {} images ({} … {})", many.len(), many[0], many[many.len() - 1]),
        })
    }

    // The IMG viewer: the COP network's output, fitted to the pane, over a checkerboard.
    fn image_view(&mut self, ui: &egui::Ui, response: &egui::Response, painter: &egui::Painter) {
        let rect = response.rect;
        let Some(img) = cops::Image::from_mesh(&self.current_mesh) else {
            let message = if self.view_displays().is_empty() { "Add a COP Network, open it and build a composite" } else { "Nothing to show: check the display node's errors" };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, message, egui::FontId::proportional(15.0), egui::Color32::from_gray(170));
            return;
        };
        let raw = &self.current_mesh.point_attributes[cops::RGBA];
        let key = (img.w, img.h, raw.as_ptr() as usize, raw.len());
        if self.cop_texture.as_ref().is_none_or(|(k, _)| *k != key) {
            let pixels: Vec<egui::Color32> = img
                .px
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (x, y) = (i % img.w, i / img.w);
                    let check = if (x / 8 + y / 8) % 2 == 0 { 0.22 } else { 0.32 };
                    let a = p[3].clamp(0.0, 1.0);
                    let c = |k: usize| ((cops::linear_to_srgb(p[k]) * a + check * (1.0 - a)).clamp(0.0, 1.0) * 255.0) as u8;
                    egui::Color32::from_rgb(c(0), c(1), c(2))
                })
                .collect();
            let image = egui::ColorImage { size: [img.w, img.h], pixels, source_size: egui::vec2(img.w as f32, img.h as f32) };
            let texture = ui.ctx().load_texture("cop_view", image, egui::TextureOptions::LINEAR);
            self.cop_texture = Some((key, texture));
        }
        let Some((_, texture)) = &self.cop_texture else { return };
        let scale = ((rect.width() - 24.0) / img.w as f32).min((rect.height() - 48.0) / img.h as f32).max(0.01);
        let shown = egui::Rect::from_center_size(rect.center(), egui::vec2(img.w as f32, img.h as f32) * scale);
        painter.image(texture.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        painter.rect_stroke(shown, 0.0, egui::Stroke::new(1.0, egui::Color32::from_gray(90)), egui::StrokeKind::Outside);
        let mut info = format!("{} × {}", img.w, img.h);
        if let Some(p) = response.hover_pos().filter(|p| shown.contains(*p)) {
            let (x, y) = (((p.x - shown.min.x) / scale) as usize, ((p.y - shown.min.y) / scale) as usize);
            let v = img.px[y.min(img.h - 1) * img.w + x.min(img.w - 1)];
            info += &format!("   ({x}, {y})  R {:.3}  G {:.3}  B {:.3}  A {:.3}", v[0], v[1], v[2], v[3]);
        }
        painter.text(rect.left_top() + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, info, egui::FontId::monospace(12.0), egui::Color32::from_gray(210));
    }

    fn begin_sequence(&mut self, job: SequenceJob) {
        if job.times.is_empty() {
            self.status = "Frame range is empty".into();
            return;
        }
        self.sequence = Some(job);
        self.advance_sequence();
    }

    // Starts the next frame of the running sequence once the previous one has finished.
    fn advance_sequence(&mut self) {
        if self.moonray_rx.is_some() {
            return;
        }
        let Some(job) = self.sequence.as_mut() else {
            return;
        };
        let total = job.times.len();
        if job.next >= total {
            let first = PathBuf::from(frame_output(&job.output, 1));
            let folder = if first.is_relative() {
                std::env::temp_dir().join("proceduralfx_sequence")
            } else {
                first.parent().map(PathBuf::from).unwrap_or_default()
            };
            self.status = format!("Sequence complete: {total} frames in {}", folder.display());
            self.sequence = None;
            self.render_at = None;
            return;
        }
        let index = job.next;
        job.next += 1;
        let time = job.times[index];
        let (executable, arguments, output) = (job.executable.clone(), job.arguments.clone(), frame_output(&job.output, index + 1));
        let (width, height, samples) = (job.width, job.height, job.samples);
        self.render_opts = job.opts.clone();
        self.render_at = Some(time);
        self.spawn_moonray_render(executable, arguments, output, width, height, samples);
        if self.moonray_rx.is_none() {
            // spawn_moonray_render rejected the settings and set the status.
            self.sequence = None;
            self.render_at = None;
            return;
        }
        self.status = format!("Rendering frame {}/{total} (t = {time:.3})...", index + 1);
    }

    fn export_scene_rdla(
        &self,
        path: &std::path::Path,
        scene: &RenderScene,
        width: usize,
        height: usize,
        samples: usize,
    ) -> Result<(), String> {
        use std::fs::File;
        use std::io::Write;

        let mut f = File::create(path).map_err(|e| format!("Failed to create RDLA file {}: {}", path.display(), e))?;
        // Best-effort RDLA writer: one RdlMeshGeometry per active geometry,
        // MoonRay materials from MAT nodes, lights, and a camera.
        writeln!(f, "-- RDLA generated by ProceduralFX Studio") .map_err(|e| e.to_string())?;
        writeln!(f, "") .map_err(|e| e.to_string())?;

        let opts = &self.render_opts;
        writeln!(f, "SceneVariables {{") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_width\"] = {},", width).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_height\"] = {},", height).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"output_file\"] = \"\",") .map_err(|e| e.to_string())?;
        // MoonRay's pixel_samples is per axis, so N samples per pixel is sqrt(N) per side.
        writeln!(f, "    [\"pixel_samples\"] = {},", (samples.max(1) as f32).sqrt().ceil() as usize) .map_err(|e| e.to_string())?;
        // Adaptive sampling stops early where pixels have converged (measured: same image,
        // fewer samples); the sample count is the maximum.
        writeln!(f, "    [\"sampling_mode\"] = 2,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"min_adaptive_samples\"] = {},", (samples / 4).max(4).min(samples.max(1))) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"max_adaptive_samples\"] = {},", samples.max(1)) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"target_adaptive_error\"] = 1.0,") .map_err(|e| e.to_string())?;
        if !scene.volumes.is_empty() {
            // Finer ray marching and one bounce inside volumes keep smoke from rendering too dark.
            writeln!(f, "    [\"volume_quality\"] = 1.0,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"volume_indirect_samples\"] = 1,") .map_err(|e| e.to_string())?;
            // In-scattering samples dominate volume render time (each marches shadow rays).
            writeln!(f, "    [\"volume_illumination_samples\"] = {},", opts.volume_samples.max(1)) .map_err(|e| e.to_string())?;
        }
        writeln!(f, "    [\"max_depth\"] = {},", opts.max_depth) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"light_samples\"] = {},", opts.light_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"bsdf_samples\"] = {},", opts.bsdf_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        let parts = &scene.parts;
        // The viewport is left-handed and MoonRay right-handed, so z is mirrored
        // here and in the camera, and triangle winding is swapped to match.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if mesh.indices.is_empty() {
                continue;
            }
            writeln!(f, "RdlMeshGeometry(\"/scene/objects/mesh{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertex_list_0\"] = {{") .map_err(|e| e.to_string())?;
            for vertex in &mesh.vertices {
                writeln!(f, "        Vec3({:.9}, {:.9}, {:.9}),", vertex.x, vertex.y, -vertex.z).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertices_by_index\"] = {{") .map_err(|e| e.to_string())?;
            for tri in &mesh.indices {
                writeln!(f, "        {},", tri[0]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[2]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[1]).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"face_vertex_count\"] = {{") .map_err(|e| e.to_string())?;
            for _ in &mesh.indices {
                writeln!(f, "        3,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_list\"] = {{ \"tri_shape_0\" }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_count_list\"] = {{ {} }},", mesh.indices.len()) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_indices\"] = {{") .map_err(|e| e.to_string())?;
            for i in 0..mesh.indices.len() {
                writeln!(f, "        {},", i).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            // UVs per face-vertex (in the swapped winding) for image textures.
            if let Some(uvs) = sops::corner_uvs(mesh) {
                writeln!(f, "    [\"uv_list\"] = {{") .map_err(|e| e.to_string())?;
                for uv in &uvs {
                    for k in [0, 2, 1] {
                        writeln!(f, "        Vec2({:.6}, {:.6}),", uv[k][0], uv[k][1]).map_err(|e| e.to_string())?;
                    }
                }
                writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            }
            // Cd as user data, for shaders that read point colour (rate guessed from the count).
            let cd = mesh
                .point_attributes
                .get("Cd")
                .filter(|c| c.len() == mesh.vertices.len() * 3)
                .or_else(|| mesh.point_attributes.get("prim:Cd").filter(|c| c.len() == mesh.indices.len() * 3));
            // Alpha too, for shaders using point alpha.
            let alpha = mesh
                .point_attributes
                .get("Alpha")
                .filter(|a| a.len() == mesh.vertices.len())
                .or_else(|| mesh.point_attributes.get("prim:Alpha").filter(|a| a.len() == mesh.indices.len()));
            let user_data: Vec<String> = [cd.map(|_| "Cd"), alpha.map(|_| "Alpha")].into_iter().flatten().map(|key| format!("UserData(\"/scene/objects/mesh{index}_{key}\")")).collect();
            if !user_data.is_empty() {
                writeln!(f, "    [\"primitive_attributes\"] = {{ {} }},", user_data.join(", ")) .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    [\"is_subd\"] = false") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            if let Some(cd) = cd {
                let mut body = format!("UserData(\"/scene/objects/mesh{index}_Cd\") {{\n    [\"color_key\"] = \"Cd\",\n    [\"color_values_0\"] = {{\n");
                for c in cd.chunks(3) {
                    body.push_str(&format!("        Rgb({:.4}, {:.4}, {:.4}),\n", c[0], c[1], c[2]));
                }
                body.push_str("    }\n}\n\n");
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            }
            if let Some(alpha) = alpha {
                let mut body = format!("UserData(\"/scene/objects/mesh{index}_Alpha\") {{\n    [\"float_key\"] = \"Alpha\",\n    [\"float_values_0\"] = {{\n");
                for a in alpha {
                    body.push_str(&format!("        {a:.4},\n"));
                }
                body.push_str("    }\n}\n\n");
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            }
        }

        // Hair and other curves: linear RdlCurveGeometry, radius from the width attribute.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            let curves = groom::curves(mesh);
            if curves.is_empty() {
                continue;
            }
            let width = mesh.point_attributes.get("width").filter(|w| w.len() == mesh.vertices.len());
            let mut body = format!("RdlCurveGeometry(\"/scene/objects/curves{index}\") {{\n    [\"curve_type\"] = 0,\n    [\"vertex_list_0\"] = {{\n");
            for range in &curves {
                for v in &mesh.vertices[range.clone()] {
                    body.push_str(&format!("        Vec3({:.7}, {:.7}, {:.7}),\n", v.x, v.y, -v.z));
                }
            }
            body.push_str("    },\n    [\"curves_vertex_count\"] = {\n");
            for range in &curves {
                body.push_str(&format!("        {},\n", range.len()));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for range in &curves {
                for i in range.clone() {
                    body.push_str(&format!("        {:.6},\n", width.map_or(0.005, |w| w[i] * 0.5).max(1e-5)));
                }
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Whitewater and other marked points: spheres of radius pscale.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if !renders_points(mesh) {
                continue;
            }
            let pscale = mesh.point_attributes.get("pscale").filter(|p| p.len() == mesh.vertices.len());
            let mut body = format!("RdlPointGeometry(\"/scene/objects/points{index}\") {{\n    [\"vertex_list_0\"] = {{\n");
            for v in &mesh.vertices {
                body.push_str(&format!("        Vec3({:.6}, {:.6}, {:.6}),\n", v.x, v.y, -v.z));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for i in 0..mesh.vertices.len() {
                body.push_str(&format!("        {:.6},\n", pscale.map_or(0.01, |p| p[i]).max(1e-5)));
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Fog volumes: VdbGeometry reading the written .vdb, shaded by VdbVolume.
        for (index, vol) in scene.volumes.iter().enumerate() {
            writeln!(f, "VdbGeometry(\"/scene/volumes/volume{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"model\"] = \"{}\",", vol.file.replace('\\', "/")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"density_grid\"] = \"density\",") .map_err(|e| e.to_string())?;
            if vol.emission.is_some() {
                writeln!(f, "    [\"emission_grid\"] = \"emission\",") .map_err(|e| e.to_string())?;
                // Sample the fire densely so flames render smooth instead of speckled.
                writeln!(f, "    [\"emission_sample_rate\"] = 1.0,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            let emission = vol.emission.unwrap_or(0.0);
            writeln!(f, "VdbVolume(\"/scene/volumes/shader{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"opacity_gain_mult\"] = Rgb({0:.5}, {0:.5}, {0:.5}),", vol.extinction) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", vol.albedo[0], vol.albedo[1], vol.albedo[2]) .map_err(|e| e.to_string())?;
            let tint = vol.emission_tint.map(|t| t * emission);
            writeln!(f, "    [\"incandescence_gain_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", tint[0], tint[1], tint[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"anisotropy\"] = {:.4}", vol.anisotropy) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
        }

        // Each part renders as a mesh, curves, or both.
        let part_geometry: Vec<Vec<String>> = parts
            .iter()
            .enumerate()
            .map(|(index, (mesh, _))| {
                let mut refs = Vec::new();
                if !mesh.indices.is_empty() {
                    refs.push(format!("RdlMeshGeometry(\"/scene/objects/mesh{index}\")"));
                }
                if groom::has_curves(mesh) && !groom::curves(mesh).is_empty() {
                    refs.push(format!("RdlCurveGeometry(\"/scene/objects/curves{index}\")"));
                }
                if renders_points(mesh) {
                    refs.push(format!("RdlPointGeometry(\"/scene/objects/points{index}\")"));
                }
                refs
            })
            .collect();
        writeln!(f, "GeometrySet(\"/scene/geometrysets/GeometrySet1\") {{") .map_err(|e| e.to_string())?;
        for geometry in part_geometry.iter().flatten() {
            writeln!(f, "    {geometry},") .map_err(|e| e.to_string())?;
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    VdbGeometry(\"/scene/volumes/volume{index}\"),") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Lights come from active Light nodes (or the default key/fill rig).
        // z is mirrored like the mesh; MoonRay lights emit along local -Z.
        let mat4 = |x: Vec3, y: Vec3, z: Vec3, p: Vec3| {
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
                x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z, p.x, p.y, p.z
            );
            format!("blur({m}, {m})")
        };
        let mut light_names: Vec<String> = Vec::new();
        for (index, (position, direction, color, intensity, kind)) in scene.lights.iter().enumerate() {
            let name = format!("/scene/lights/light{index}");
            let rgb = format!("Rgb({:.4}, {:.4}, {:.4})", color[0], color[1], color[2]);
            if *kind == 1 {
                let xform = mat4(
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                    Vec3::new(position.x, position.y, -position.z),
                );
                writeln!(f, "SphereLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * POINT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"radius\"] = 0.1,") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"visible_in_camera\"] = \"force off\",") .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            } else {
                let z = Vec3::new(-direction.x, -direction.y, direction.z).normalize();
                let hint = if z.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                let x = hint.cross(z).normalize();
                let y = z.cross(x);
                let xform = mat4(x, y, z, Vec3::zero());
                writeln!(f, "DistantLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * DISTANT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"angular_extent\"] = {},", if *kind == 2 { 10.0 } else { 0.53 }) .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            }
            light_names.push(format!("{}(\"{name}\")", if *kind == 1 { "SphereLight" } else { "DistantLight" }));
        }

        // One MoonRay material per MAT node used by a geometry, plus a neutral default.
        let (default_class, default_body) = rdla_material("/scene/materials/default", 0, [0.8; 3], 0.5, 0.5);
        write!(f, "{default_body}\n") .map_err(|e| e.to_string())?;
        let mut written: Vec<NodeId> = Vec::new();
        let mut layer_materials: Vec<String> = Vec::new();
        for (index, (_, material)) in parts.iter().enumerate() {
            let node = match material {
                PartMaterial::Node(id) => self.graph.nodes.get(id).map(|n| (*id, &n.kind)),
                _ => None,
            };
            let reference = match (material, node) {
                (_, Some((id, NodeKind::Shader(v)))) if v.table != shading::Table::Pyro => {
                    let name = format!("/scene/materials/mat{id}");
                    let mut objects = String::new();
                    let (class, body) = match v.table {
                        shading::Table::Toon => shading::toon_rdla(&name, v, &mut objects),
                        shading::Table::Hair => shading::hair_rdla(&name, v),
                        _ => shading::principled_rdla(&name, v, &HashMap::new(), &mut objects),
                    };
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{objects}{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::MaterialBuilder))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let output = self.graph.builder_output(id).ok_or("Material Builder: add a Surface Output node inside it")?;
                    let nodes = self.graph.builder_nodes(id);
                    let (class, text) = shading::compile_builder(&name, &nodes, |to, slot| self.graph.input_of(to, slot), output)
                        .map_err(|e| format!("Material Builder {}: {e}", self.graph.nodes[&id].title(id)))?;
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{text}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::Material { kind, color, roughness, value }))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let (class, body) = rdla_material(&name, *kind, *color, *roughness, *value);
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (PartMaterial::Preview { diffuse, roughness, metallic, opacity, emissive, ior }, _) => {
                    // A USD preview surface mapped onto the closest MAT material type.
                    let name = format!("/scene/materials/usd{index}");
                    let emission = emissive.iter().cloned().fold(0.0, f32::max);
                    let (kind, color, value) = if emission > 0.0 {
                        (3, emissive.map(|c| c / emission), emission)
                    } else if *metallic > 0.5 {
                        (1, *diffuse, 0.5)
                    } else if *opacity < 0.5 {
                        (2, *diffuse, *ior)
                    } else {
                        (0, *diffuse, 0.5)
                    };
                    let (class, body) = rdla_material(&name, kind, color, *roughness, value);
                    write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    format!("{class}(\"{name}\")")
                }
                _ => format!("{default_class}(\"/scene/materials/default\")"),
            };
            layer_materials.push(reference);
        }

        // An HDRI environment light replaces the uniform ambient environment below.
        let hdri = self.environment_light();
        if let Some((file, color, rotation, visible)) = &hdri {
            envlight::load(file).map_err(|e| format!("Environment light: {e}"))?;
            let r = envlight::rotation_rows(*rotation);
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, 0,0,0,1)",
                r[0].x, r[0].y, r[0].z, r[1].x, r[1].y, r[1].z, r[2].x, r[2].y, r[2].z
            );
            writeln!(f, "EnvLight(\"/scene/lights/hdri\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"texture\"] = \"{}\",", file.replace('\\', "/").replace('"', "\\\"")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", color[0], color[1], color[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"visible_in_camera\"] = {},", if *visible { 1 } else { 0 }) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"node_xform\"] = blur({m}, {m})") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/hdri\")".to_string());
        }
        // The viewport's ambient term becomes a dim environment light (decoded from sRGB).
        let ambient = self.ambient_light.max(0.0).powf(2.2);
        let environment: [f32; 3] = std::array::from_fn(|i| opts.background[i] + scene.environment.map_or(0.0, |e| e[i]) + ambient);
        if hdri.is_none() && environment.iter().any(|c| *c > 0.0) {
            writeln!(f, "EnvLight(\"/scene/lights/env\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", environment[0], environment[1], environment[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/env\")".to_string());
        }

        writeln!(f, "LightSet(\"/scene/lightsets/LightSet1\") {{") .map_err(|e| e.to_string())?;
        for light in &light_names {
            writeln!(f, "    {light},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        writeln!(f, "Layer(\"/scene/layers/Layer1\") {{") .map_err(|e| e.to_string())?;
        for (geometry, material) in part_geometry.iter().zip(&layer_materials) {
            for geometry in geometry {
                writeln!(f, "    {{{geometry}, \"\", {material}, LightSet(\"/scene/lightsets/LightSet1\"), undef(), undef(), undef(), undef()}},") .map_err(|e| e.to_string())?;
            }
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    {{VdbGeometry(\"/scene/volumes/volume{index}\"), \"\", LightSet(\"/scene/lightsets/LightSet1\"), VdbVolume(\"/scene/volumes/shader{index}\")}},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Camera from the stage, the selected/chosen camera node, or the viewport.
        let (rig, _) = self.resolve_render_camera();
        let (sx, cx) = rig.rot_x.sin_cos();
        let (sy, cy) = rig.rot_y.sin_cos();
        let mut right = Vec3::new(cy, 0.0, -sy);
        let mut up = Vec3::new(-sx * sy, cx, -sx * cy);
        let mut fwd = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = rig.zoom / self.viewport_focal_px.max(1.0);
        let target = rig
            .target
            .sub(right.scale(rig.pan[0] * pan_scale))
            .add(up.scale(rig.pan[1] * pan_scale));
        let mut eye = target.sub(fwd.scale(rig.zoom));
        let mut lens = None;
        if let Some((stage_eye, stage_right, stage_up, stage_fwd, focal, aperture)) = scene.camera {
            (eye, right, up, fwd) = (stage_eye, stage_right, stage_up, stage_fwd);
            lens = Some((focal, aperture));
        }
        // z is mirrored (see mesh export); MoonRay cameras look down local -Z.
        let xform = format!(
            "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
            right.x, right.y, -right.z,
            up.x, up.y, -up.z,
            -fwd.x, -fwd.y, fwd.z,
            eye.x, eye.y, -eye.z,
        );
        let min_dim = width.min(height).max(1) as f32;
        let aspect_w = width.max(1) as f32;
        let orthographic = rig.orthographic && lens.is_none();
        writeln!(f, "{}(\"/scene/cameras/Camera1\") {{", if orthographic { "OrthographicCamera" } else { "PerspectiveCamera" }).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"node_xform\"] = blur({xform}, {xform}),") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"near\"] = 0.001,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"far\"] = 100000,") .map_err(|e| e.to_string())?;
        if let Some((focal, aperture)) = lens {
            writeln!(f, "    [\"film_width_aperture\"] = {aperture:.6},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal:.6}, {focal:.6})") .map_err(|e| e.to_string())?;
        } else if rig.orthographic {
            writeln!(f, "    [\"film_width_aperture\"] = {:.6}", rig.zoom / rig.focal * aspect_w / min_dim) .map_err(|e| e.to_string())?;
        } else {
            // Match the viewport's field of view along the smaller image dimension.
            let focal_mm = 24.0 * min_dim * rig.focal / aspect_w;
            writeln!(f, "    [\"film_width_aperture\"] = 24,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal_mm:.6}, {focal_mm:.6})") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}") .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn set_camera_view(&mut self, rot_x: f32, rot_y: f32) {
        self.cam_rot_x = rot_x;
        self.cam_rot_y = rot_y;
        self.cam_pan_x = 0.0;
        self.cam_pan_y = 0.0;
        self.frame_all();
    }

    // Geometry node being edited, if it still exists.
    fn network(&self) -> Option<NodeId> {
        self.current_network.filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.kind.is_container()))
    }

    // The network containing the open one (a SOP Solver's geometry), or None at the top.
    fn parent_network(&self) -> Option<NodeId> {
        self.network()
            .and_then(|id| self.graph.nodes.get(&id))
            .and_then(|n| n.parent)
            .filter(|p| self.graph.nodes.get(p).is_some_and(|n| n.kind.is_container()))
    }

    // Parent assigned to nodes created in the visible network.
    fn view_parent(&self) -> Option<NodeId> {
        self.network().filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.context == self.context))
    }

    // Nodes cooked into the viewport: the open geometry's display, else every top-level geometry.
    fn view_displays(&self) -> Vec<NodeId> {
        if self.context == AppContext::Stage {
            return self.graph.display_of(STAGE_NETWORK).into_iter().collect();
        }
        if self.context == AppContext::Img {
            // The open COP network's output, else the first COP network's.
            let net = self.network().or_else(|| {
                let mut nets: Vec<NodeId> = self.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::CopNetwork)).map(|(id, _)| *id).collect();
                nets.sort_unstable();
                nets.first().copied()
            });
            return net.and_then(|n| self.graph.display_of(n)).into_iter().collect();
        }
        if self.context == AppContext::Tasks {
            return self.graph.geometry_ids();
        }
        match self.network() {
            Some(geo) => self.graph.display_of(geo).into_iter().collect(),
            None => self.graph.geometry_ids(),
        }
    }

    // Opens `network` (None: the top level). The editor takes the context its nodes
    // live in: OBJ for geometry networks, MAT for a Material Builder; going back to
    // the top stays in the context of the network being left.
    fn enter_network(&mut self, network: Option<NodeId>) {
        let context_of = |id: NodeId| self.graph.nodes.get(&id).map(|n| n.context);
        self.context = match network {
            Some(id) => context_of(id),
            None => self.network().and_then(context_of),
        }
        .unwrap_or(AppContext::Obj);
        self.current_network = network;
        self.selected = None;
        self.graph_pan = egui::Vec2::ZERO;
        self.clear_component_selection();
        self.dirty = true;
    }

    fn templates_for_view(&self) -> Vec<(&'static str, Vec<NodeKind>)> {
        let in_obj = self.context == AppContext::Obj;
        let top_obj = in_obj && self.network().is_none();
        let in_solver = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::SopSolver { .. })));
        let in_vop = self.network().is_some_and(|id| {
            matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. })) || matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::Pop(p)) if p.op == pop::VOP)
        });
        let in_popnet = self.network().is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::PopNet { .. })));
        let in_subnet = self.network().is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::Subnet { .. })));
        let in_builder = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MaterialBuilder)));
        let in_cop = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::CopNetwork)));
        let in_top = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::TopNetwork)));
        let context = self.context;
        NodeKind::templates()
            .into_iter()
            .filter_map(|(category, kinds)| {
                let kinds: Vec<NodeKind> = kinds
                    .into_iter()
                    .filter(|k| match context {
                        // Inside a Material Builder only shader nodes; shader nodes nowhere else.
                        AppContext::Mat if in_builder => matches!(k, NodeKind::ShaderNode(_)),
                        AppContext::Mat => k.is_material(),
                        AppContext::Out => k.is_render(),
                        AppContext::Stage => k.is_lop(),
                        // IMG and TASKS: only their network node at the top, and its nodes inside.
                        AppContext::Img if in_cop => matches!(k, NodeKind::Cop(_)),
                        AppContext::Img => matches!(k, NodeKind::CopNetwork),
                        AppContext::Tasks if in_top => matches!(k, NodeKind::Top(_)),
                        AppContext::Tasks => matches!(k, NodeKind::TopNetwork),
                        AppContext::Obj if k.is_lop() => false,
                        // Inside an Attribute VOP only VOPs; VOPs nowhere else.
                        AppContext::Obj if in_vop => matches!(k, NodeKind::Vop(_)),
                        AppContext::Obj if matches!(k, NodeKind::Vop(_)) => false,
                        // Inside a POP Network only POPs and Merge; POPs nowhere else.
                        AppContext::Obj if in_popnet => matches!(k, NodeKind::Pop(_) | NodeKind::Merge),
                        AppContext::Obj if matches!(k, NodeKind::Pop(_)) => false,
                        // The POP Network replaces it; scenes that have one still work.
                        AppContext::Obj if matches!(k, NodeKind::Particles { .. }) => false,
                        AppContext::Obj if top_obj => k.is_object(),
                        AppContext::Obj if matches!(k, NodeKind::SolverInput { .. }) => in_solver,
                        AppContext::Obj if matches!(k, NodeKind::SubnetInput { .. }) => in_subnet,
                        AppContext::Obj => {
                            !k.is_object()
                                && !k.is_material()
                                && !k.is_render()
                                && !matches!(k, NodeKind::ShaderNode(_) | NodeKind::CopNetwork | NodeKind::Cop(_) | NodeKind::TopNetwork | NodeKind::Top(_))
                        }
                    })
                    .collect();
                (!kinds.is_empty()).then_some((category, kinds))
            })
            .chain(
                // The asset library, wherever SOPs go.
                (context == AppContext::Obj && !top_obj && !in_vop && !in_popnet && !self.asset_library.is_empty()).then(|| {
                    let kinds = self
                        .asset_library
                        .iter()
                        .map(|e| NodeKind::Subnet { values: Vec::new(), spec: assets::Spec { asset: Some((e.name.clone(), e.version)), description: e.description.clone(), params: Vec::new() } })
                        .collect();
                    ("Assets", kinds)
                }),
            )
            .collect()
    }

    // (position, travel direction, color, intensity, kind) for every active Light node,
    // or the viewport's default key/fill rig when the scene has none.
    // The active (first, not bypassed) environment light with a file: (file, color * intensity, rotation, visible).
    fn environment_light(&self) -> Option<(String, [f32; 3], f32, bool)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().find_map(|id| {
            let node = &self.graph.nodes[&id];
            match &node.kind {
                NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } if !node.bypass && node.parent.is_none() && !file.trim().is_empty() => {
                    Some((file.trim().to_owned(), color.map(|c| c * intensity), *rotation, *visible))
                }
                _ => None,
            }
        })
    }

    // The scene's lights for shading smoke in the real-time renderer: none when it has no
    // light nodes or environment light, so smoke keeps the solver's own overhead shading.
    fn volume_lights(&self) -> Vec<sims::VolumeLight> {
        let has_lights = self.graph.nodes.values().any(|n| !n.bypass && n.parent.is_none() && matches!(n.kind, NodeKind::Light { .. }));
        if !has_lights && self.environment_light().is_none() {
            return Vec::new();
        }
        let mut lights: Vec<sims::VolumeLight> = self
            .scene_lights()
            .into_iter()
            .map(|(position, direction, color, intensity, kind)| match kind {
                1 => sims::VolumeLight::Sphere { position, color, intensity, falloff: POINT_LIGHT_FALLOFF },
                2 => sims::VolumeLight::Dome { color, intensity },
                _ => sims::VolumeLight::Distant { to_light: direction.scale(-1.0), color, intensity },
            })
            .collect();
        if self.environment_light().is_some() {
            lights.push(sims::VolumeLight::Dome { color: [1.0; 3], intensity: 1.0 });
        }
        lights
    }

    fn scene_lights(&self) -> Vec<(Vec3, Vec3, [f32; 3], f32, usize)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        let lights: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let node = &self.graph.nodes[&id];
                if node.bypass || node.parent.is_some() {
                    return None;
                }
                let NodeKind::Light { position, rotation, color, intensity, kind } = &node.kind else {
                    return None;
                };
                Some((
                    Vec3::from_arr(*position),
                    Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize(),
                    *color,
                    *intensity,
                    *kind,
                ))
            })
            .collect();
        // Any light, including an HDRI, replaces the default key/fill rig.
        if !lights.is_empty() || self.environment_light().is_some() {
            return lights;
        }
        vec![
            (
                Vec3::zero(),
                Vec3::from_arr(self.key_light_dir).normalize(),
                [1.0; 3],
                self.key_light_intensity.max(0.0),
                0,
            ),
            (
                Vec3::zero(),
                Vec3::from_arr(self.fill_light_dir).normalize(),
                [1.0; 3],
                self.fill_light_intensity.max(0.0),
                2,
            ),
        ]
    }

    // Active geometries with their MAT assignment, regardless of which network the viewport shows.
    // The displayed USD stage written for hd_render; returns the camera to render
    // through (the stage's first camera with the Auto camera, else hd_render's own view).
    fn write_hydra_scene(&self, path: &std::path::Path) -> Result<Option<String>, String> {
        let display = self.graph.display_of(STAGE_NETWORK).ok_or("Render STAGE: no LOP node has the display flag in the STAGE network")?;
        let time = self.render_at.unwrap_or(self.time);
        let mut stage = (*self.graph.cook_stage(display, time, &mut HashMap::new())).clone();
        // Light the stage as the MoonRay render does: normalized lights (brightness doesn't
        // depend on their size; USD's default isn't), with the same sphere light scale.
        for prim in stage.prims.values_mut().filter(|prim| prim.is_light() && prim.type_name != "DomeLight") {
            prim.attrs.insert("normalize".into(), usd::AttrValue::Bool(true));
            if prim.type_name == "SphereLight" {
                if let Some(usd::AttrValue::Float(intensity)) = prim.attrs.get_mut("intensity") {
                    *intensity *= POINT_LIGHT_SCALE;
                }
            }
        }
        std::fs::write(path, usd::to_usda(&stage)).map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        let auto = self.render_opts.camera == RenderCamera::Auto;
        // hd_render wants the camera path relative to the stage root.
        Ok(stage.prims.iter().find(|(p, prim)| auto && prim.type_name == "Camera" && stage.is_shown(p)).map(|(p, _)| p.trim_start_matches('/').to_string()))
    }

    fn render_scene_parts(&self) -> Vec<(Mesh, Option<NodeId>)> {
        let material_of = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Geometry { material }) => *material,
            _ => None,
        };
        let active: Vec<NodeId> = self
            .graph
            .geometry_ids()
            .into_iter()
            .filter(|id| !self.graph.nodes[id].bypass)
            .collect();
        let time = self.render_at.unwrap_or(self.time);
        if self.render_at.is_none() && active.len() == 1 && self.network().is_none() && self.context != AppContext::Stage {
            return vec![(self.current_mesh.clone(), material_of(active[0]))];
        }
        let mut cache = self.valid_cache(&self.graph.cook_fingerprints(scene_fingerprint(&self.graph.serialize())));
        active
            .into_iter()
            .map(|id| {
                let mesh = self.graph.cook(id, time, &mut cache);
                (if packed::is_packed(&mesh) { packed::unpack(&mesh, true) } else { mesh }, material_of(id))
            })
            .filter(|(mesh, _)| !mesh.indices.is_empty() || volume::is_volume(mesh) || groom::has_curves(mesh) || renders_points(mesh))
            .collect()
    }

    // The scene to render: the displayed USD stage when the render options ask for it, else the OBJ objects.
    fn render_scene(&self) -> Result<RenderScene, String> {
        if !self.render_opts.stage {
            let mut parts = Vec::new();
            let mut volumes = Vec::new();
            for (index, (mesh, material)) in self.render_scene_parts().into_iter().enumerate() {
                if !volume::is_volume(&mesh) {
                    // Material SOP assignments split the object into one part per material.
                    let is_material = |m: NodeId| self.graph.nodes.get(&m).is_some_and(|n| n.kind.is_material());
                    for (part, m) in split_by_material(mesh, material, is_material) {
                        parts.push((part, m.map_or(PartMaterial::Default, PartMaterial::Node)));
                    }
                    continue;
                }
                let material = material.map_or(PartMaterial::Default, PartMaterial::Node);
                let grid = volume::from_mesh(&mesh)?;
                if grid.class == volume::Class::LevelSet {
                    // Level sets render as their surface.
                    parts.push((volume::to_polygons(&grid, 0.0), material));
                } else {
                    let mut volume = write_render_volume(&mesh, index)?;
                    if let PartMaterial::Node(m) = material {
                        if let Some(NodeKind::Shader(v)) = self.graph.nodes.get(&m).map(|n| &n.kind).filter(|k| matches!(k, NodeKind::Shader(v) if v.table == shading::Table::Pyro)) {
                            volume.extinction *= v.float("smoke_density");
                            volume.albedo = v.color("smoke_color");
                            volume.emission = volume.emission.map(|e| e * v.float("fire_intensity"));
                            volume.emission_tint = v.color("fire_color");
                            volume.anisotropy = v.float("anisotropy");
                        }
                    }
                    volumes.push(volume);
                }
            }
            return Ok(RenderScene { parts, lights: self.scene_lights(), environment: None, camera: None, volumes });
        }
        let display = self.graph.display_of(STAGE_NETWORK).ok_or("Render STAGE: no LOP node has the display flag in the STAGE network")?;
        let time = self.render_at.unwrap_or(self.time);
        let stage = self.graph.cook_stage(display, time, &mut HashMap::new());
        let parts = stage
            .flatten()
            .into_iter()
            .filter(|(_, mesh, _)| !mesh.indices.is_empty())
            .map(|(_, mesh, material)| {
                let material = material.as_deref().and_then(|path| stage.material(path)).map_or(PartMaterial::Default, |m| {
                    match m.material_node.filter(|id| self.graph.nodes.contains_key(id)) {
                        Some(node) => PartMaterial::Node(node),
                        None => PartMaterial::Preview {
                            diffuse: m.vec3("diffuseColor").unwrap_or([0.18; 3]),
                            roughness: m.float("roughness").unwrap_or(0.5),
                            metallic: m.float("metallic").unwrap_or(0.0),
                            opacity: m.float("opacity").unwrap_or(1.0),
                            emissive: m.vec3("emissiveColor").unwrap_or([0.0; 3]),
                            ior: m.float("ior").unwrap_or(1.5),
                        },
                    }
                });
                (mesh, material)
            })
            .collect();
        let mut lights = Vec::new();
        let mut environment: Option<[f32; 3]> = None;
        let mut camera = None;
        for (path, prim) in &stage.prims {
            if !stage.is_shown(path) {
                continue;
            }
            let world = stage.world_xform(path);
            let colour = prim.vec3("color").unwrap_or([1.0; 3]);
            let intensity = prim.float("intensity").unwrap_or(1.0) * 2f32.powf(prim.float("exposure").unwrap_or(0.0));
            match prim.type_name.as_str() {
                "DomeLight" => {
                    let add = colour.map(|c| c * intensity);
                    let current = environment.unwrap_or([0.0; 3]);
                    environment = Some(std::array::from_fn(|i| current[i] + add[i]));
                }
                "DistantLight" => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 0)),
                kind if kind.ends_with("Light") => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 1)),
                "Camera" if camera.is_none() && self.render_opts.camera == RenderCamera::Auto => {
                    camera = Some((
                        usd::row(&world, 3),
                        usd::row(&world, 0).normalize(),
                        usd::row(&world, 1).normalize(),
                        usd::row(&world, 2).normalize(),
                        prim.float("focalLength").unwrap_or(50.0),
                        prim.float("horizontalAperture").unwrap_or(36.0),
                    ));
                }
                _ => {}
            }
        }
        if lights.is_empty() && environment.is_none() {
            lights = self.scene_lights();
        }
        Ok(RenderScene { parts, lights, environment, camera, volumes: Vec::new() })
    }

    // What the viewport's lit surfaces depend on besides the camera: the GPU viewport
    // keeps them uploaded while this stays the same.
    fn viewport_scene_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        (self.mesh_revision, self.render_mode as u8, self.ambient_light.to_bits()).hash(&mut h);
        for (position, direction, color, intensity, kind) in self.scene_lights() {
            for v in [position.x, position.y, position.z, direction.x, direction.y, direction.z, color[0], color[1], color[2], intensity] {
                v.to_bits().hash(&mut h);
            }
            kind.hash(&mut h);
        }
        if let Some((file, scale, rotation, shown)) = self.environment_light() {
            (file, shown).hash(&mut h);
            for v in scale.into_iter().chain([rotation]) {
                v.to_bits().hash(&mut h);
            }
        }
        if self.isolate_selection {
            (self.selection_mode as u8, &self.selected_vertices, &self.selected_triangles, &self.selected_edges).hash(&mut h);
        }
        // Toon Shader settings shade the faces without changing the mesh.
        for (id, node) in &self.graph.nodes {
            if let NodeKind::Shader(v) = &node.kind {
                if v.table == shading::Table::Toon {
                    id.hash(&mut h);
                    v.values.iter().for_each(|x| x.to_bits().hash(&mut h));
                }
            }
        }
        h.finish()
    }

    // The cached results still good for the scene as it is: a cook starts from these.
    fn valid_cache(&self, fingerprints: &HashMap<NodeId, u64>) -> HashMap<NodeId, Mesh> {
        self.static_cache.iter().filter(|(id, (f, _))| fingerprints.get(id) == Some(f)).map(|(id, (_, m))| (*id, m.clone())).collect()
    }

    fn rebuild_graph(&mut self) {
        // While picking a group the viewport keeps showing the node's input.
        if self.group_pick.is_some() {
            return;
        }
        self.cook_generation += 1;
        let t0 = Instant::now();
        // Static nodes stay cached until the network changes; only
        // time-dependent branches recook during playback.
        let key = self.graph.serialize();
        if key != self.cache_key {
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.simulation_rx = None;
            self.simulation_cancel = None;
            self.simulation_key.clear();
            self.cancelled_simulation_key.clear();
            self.frame_cache.clear();
            self.frame_cache_order.clear();
            self.frame_cache_hits = 0;
            self.frame_cache_misses = 0;
            self.cache_key = key;
        }
        let fingerprints = self.graph.cook_fingerprints(scene_fingerprint(&self.cache_key));
        let mut cache = self.valid_cache(&fingerprints);
        let displays = self.view_displays();
        let frame_key = format!(
            "{}|display={:?}|time={:08x}",
            self.cache_key,
            displays,
            self.time.to_bits()
        );
        if self.simulation_rx.is_some() && self.simulation_key != frame_key {
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.simulation_rx = None;
            self.simulation_cancel = None;
            self.simulation_key.clear();
        }
        if let Some(mesh) = self.frame_cache.get(&frame_key) {
            self.current_mesh = mesh.clone();
            self.mesh_revision += 1;
            self.frame_cache_hits += 1;
            // Routine, so it doesn't replace a message worth reading (a finished export, say).
            if self.status.is_empty() || self.status.starts_with("Frame from cache") {
                self.status = format!("Frame from cache · {} cached frames", self.frame_cache.len());
            }
        } else if displays.iter().any(|id| self.graph.time_dependent(*id)) {
            self.frame_cache_misses += 1;
            if self.simulation_key != frame_key
                && self.cancelled_simulation_key != frame_key
            {
                if let Some(cancel) = &self.simulation_cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                let graph = self.graph.clone();
                let worker_displays = displays.clone();
                let time = self.time;
                let (tx, rx) = mpsc::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                std::thread::spawn(move || {
                    FLIP_CANCEL.with(|flag| *flag.borrow_mut() = Some(worker_cancel));
                    let mut worker_cache = HashMap::new();
                    let mesh = graph.cook_display(&worker_displays, time, &mut worker_cache);
                    let _ = tx.send(mesh);
                });
                self.simulation_rx = Some(rx);
                self.simulation_cancel = Some(cancel);
                self.simulation_key = frame_key;
                self.cancelled_simulation_key.clear();
                self.status = format!("{}… press Escape to cancel", self.graph.cooking_label(&displays));
            } else if let Some(rx) = &self.simulation_rx {
                match rx.try_recv() {
                    Ok(mesh) => {
                        self.current_mesh = mesh.clone();
                        self.mesh_revision += 1;
                        const MAX_FRAME_CACHE: usize = 120;
                        if self.frame_cache.len() >= MAX_FRAME_CACHE {
                            if let Some(oldest) = self.frame_cache_order.pop_front() {
                                self.frame_cache.remove(&oldest);
                            }
                        }
                        self.frame_cache.insert(frame_key.clone(), mesh);
                        self.frame_cache_order.push_back(frame_key);
                        self.simulation_rx = None;
                        self.simulation_cancel = None;
                        self.simulation_key.clear();
                        self.status.clear();
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.simulation_rx = None;
                        self.simulation_cancel = None;
                        self.simulation_key.clear();
                        self.status = "The frame failed to cook".into();
                    }
                }
            }
        } else {
            self.current_mesh = self.graph.cook_display(&displays, self.time, &mut cache);
            self.mesh_revision += 1;
            const MAX_FRAME_CACHE: usize = 120;
            if self.frame_cache.len() >= MAX_FRAME_CACHE {
                if let Some(oldest) = self.frame_cache_order.pop_front() {
                    self.frame_cache.remove(&oldest);
                }
            }
            self.frame_cache.insert(frame_key.clone(), self.current_mesh.clone());
            self.frame_cache_order.push_back(frame_key);
        }
        self.template_mesh = match self.template {
            Some(id) if self.graph.nodes.contains_key(&id) => self.graph.cook(id, self.time, &mut cache),
            _ => Mesh::default(),
        };
        self.node_stats = cache
            .iter()
            .map(|(id, m)| (*id, (m.vertices.len(), m.indices.len())))
            .collect();
        // Node Info shows what this cook made (or cooks on its own when it needs to).
        let shown = [self.inspect_node, self.info_hold.map(|h| h.0)];
        self.info_mesh = shown.into_iter().flatten().find_map(|id| cache.get(&id).map(|m| (id, self.time, m.clone())));
        // Keep what's still good (nodes not cooked this time too), forget what isn't.
        self.static_cache.retain(|id, (f, _)| fingerprints.get(id) == Some(f));
        for (id, m) in cache {
            if let Some(&f) = fingerprints.get(&id).filter(|_| !self.graph.time_dependent(id)) {
                self.static_cache.entry(id).or_insert((f, m));
            }
        }
        self.cook_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }

    fn frame_all(&mut self) {
        if let Some((lo, hi)) = self.current_mesh.bounds() {
            self.cam_target = lo.add(hi).scale(0.5);
            self.cam_zoom = (hi.sub(lo).length() * 1.4).clamp(MIN_ZOOM, 200.0);
            self.cam_pan_x = 0.0;
            self.cam_pan_y = 0.0;
        }
    }

    fn load_state(&mut self, s: String) {
        self.graph = Graph::deserialize(&s);
        self.last_state = s;
        if self.selected.map_or(false, |id| !self.graph.nodes.contains_key(&id)) {
            self.selected = None;
        }
        self.dragging_link = None;
        self.dirty = true;
    }

    fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.last_state.clone());
            self.load_state(prev);
        }
    }

    fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.last_state.clone());
            self.load_state(next);
        }
    }

    fn create_blast_from_selection(&mut self) -> bool {
        let Some(source) = self.network().and_then(|geo| self.graph.display_of(geo)) else {
            return false;
        };
        let (component, mut indices) = match self.selection_mode {
            SelectionMode::Vertex => (0, self.selected_vertices.clone()),
            SelectionMode::Triangle => (1, self.selected_triangles.clone()),
            SelectionMode::Edge => {
                let mut edges = Vec::new();
                let mut edge_ids = HashMap::new();
                for tri in &self.current_mesh.indices {
                    for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                        let edge = if a < b { (a, b) } else { (b, a) };
                        if !edge_ids.contains_key(&edge) {
                            let id = edges.len();
                            edges.push(edge);
                            edge_ids.insert(edge, id);
                        }
                    }
                }
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                (2, edges.into_iter().enumerate()
                    .filter_map(|(id, edge)| selected.contains(&edge).then_some(id))
                    .collect())
            }
        };
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return false;
        }
        let pos = self.graph.nodes.get(&source)
            .map_or(egui::pos2(40.0, 40.0), |node| node.pos + egui::vec2(0.0, 80.0));
        let blast = self.graph.add(
            NodeKind::Blast { component, indices, invert: false },
            pos,
        );
        let network = self.network();
        if let Some(n) = self.graph.nodes.get_mut(&blast) {
            n.context = self.context;
            n.parent = network;
        }
        self.graph.connect(source, blast, 0);
        self.graph.set_display(blast);
        self.selected = Some(blast);
        self.clear_component_selection();
        self.status = "Blast node created from viewport selection".into();
        self.dirty = true;
        true
    }

    // A new SOP Solver starts like Houdini's: Prev_Frame and Input_1 nodes, displaying Prev_Frame.
    fn populate_new_node(&mut self, id: NodeId) {
        // A Subnet: an asset from the library fills itself; a new one gets an input wired
        // to an output to build between.
        if let Some(NodeKind::Subnet { spec, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
            if self.graph.children(id).is_empty() {
                match spec.asset.clone() {
                    Some((name, _)) => {
                        let entry = self.asset_library.iter().find(|e| e.name == name).cloned();
                        let result = entry.ok_or_else(|| "it isn't in the assets folder any more".to_owned()).and_then(|e| e.text()).and_then(|text| self.graph.fill_from_asset(id, &text));
                        self.status = match result {
                            Ok(()) => format!("Placed the {name} asset: double-click it to see inside"),
                            Err(e) => format!("Couldn't place the {name} asset: {e}"),
                        };
                    }
                    None => {
                        let input = self.graph.add(NodeKind::SubnetInput { index: 0 }, egui::pos2(40.0, 40.0));
                        let output = self.graph.add(NodeKind::Null, egui::pos2(40.0, 160.0));
                        for n in [input, output] {
                            if let Some(node) = self.graph.nodes.get_mut(&n) {
                                node.parent = Some(id);
                                node.context = AppContext::Obj;
                            }
                        }
                        if let Some(node) = self.graph.nodes.get_mut(&output) {
                            node.name = "OUT".into();
                        }
                        self.graph.connect(input, output, 0);
                        self.graph.displays.insert(id, output);
                        self.status = "Subnet: double-click it to build its network between Input_1 and OUT".into();
                    }
                }
            }
        }
        // Loops and compile blocks are Begin/End pairs: the Begin goes above the End, wired to it.
        let pair = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::ForEachEnd(s)) if s.begin.is_none() => Some(NodeKind::ForEachBegin),
            Some(NodeKind::CompileEnd { begin: None }) => Some(NodeKind::CompileBegin),
            _ => None,
        };
        if let Some(begin_kind) = pair {
            let (pos, context, parent) = {
                let n = &self.graph.nodes[&id];
                (n.pos, n.context, n.parent)
            };
            let begin = self.graph.add(begin_kind, pos);
            if let Some(n) = self.graph.nodes.get_mut(&begin) {
                n.context = context;
                n.parent = parent;
            }
            if let Some(n) = self.graph.nodes.get_mut(&id) {
                n.pos = pos + egui::vec2(0.0, 160.0);
                match &mut n.kind {
                    NodeKind::ForEachEnd(s) => s.begin = Some(begin),
                    NodeKind::CompileEnd { begin: b } => *b = Some(begin),
                    _ => {}
                }
            }
            self.graph.connect(begin, id, 0);
            self.status = "Loop created: put the nodes to repeat between the Begin and the End".into();
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_lop()) && self.graph.display_of(STAGE_NETWORK).is_none() {
            self.graph.set_display(id);
        }
        if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::PopNet { .. })) {
            // Like Houdini's: the source in the solver's post-solve input, so particles are
            // born where the source is at the end of a step; gravity after the solver.
            let mut add = |kind: NodeKind, x: f32, y: f32| {
                let child = self.graph.add(kind, egui::pos2(x, y));
                if let Some(n) = self.graph.nodes.get_mut(&child) {
                    n.context = AppContext::Obj;
                    n.parent = Some(id);
                }
                child
            };
            let object = add(NodeKind::Pop(pop::PopNode::new(pop::OBJECT)), 40.0, 20.0);
            let source = add(NodeKind::Pop(pop::PopNode::new(pop::SOURCE)), 240.0, 20.0);
            let solver = add(NodeKind::Pop(pop::PopNode::new(pop::SOLVER)), 40.0, 140.0);
            let gravity = add(NodeKind::Pop(pop::PopNode::new(pop::GRAVITY)), 40.0, 260.0);
            self.graph.connect(object, solver, 0);
            self.graph.connect(source, solver, 2);
            self.graph.connect(solver, gravity, 0);
            self.graph.set_display(gravity);
            self.status = "POP Network created: double-click it to add forces, colour and more (TAB inside lists the POPs)".into();
            return;
        }
        if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. }))
            || matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::Pop(p)) if p.op == pop::VOP)
        {
            // Like Houdini's: P read in and written back out, ready to be modified in between.
            let bind = self.graph.add(NodeKind::Vop(vop::VopNode::new(vop::BIND)), egui::pos2(40.0, 30.0));
            let export = self.graph.add(NodeKind::Vop(vop::VopNode::new(vop::EXPORT)), egui::pos2(40.0, 260.0));
            for child in [bind, export] {
                if let Some(n) = self.graph.nodes.get_mut(&child) {
                    n.context = AppContext::Obj;
                    n.parent = Some(id);
                }
            }
            self.graph.connect(bind, export, 0);
            self.status = "Attribute VOP created: double-click it to build its network (bind P → export P)".into();
        }
        if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MaterialBuilder)) {
            let output = self.graph.add(NodeKind::ShaderNode(shading::ShaderNode::new(shading::SURFACE_OUTPUT)), egui::pos2(40.0, 240.0));
            if let Some(n) = self.graph.nodes.get_mut(&output) {
                n.context = AppContext::Mat;
                n.parent = Some(id);
            }
            self.graph.set_display(output);
            self.status = "Material Builder created: double-click it and wire texture nodes into the Surface Output".into();
            return;
        }
        if !matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::SopSolver { .. })) {
            return;
        }
        for (index, x) in [(0, 40.0), (1, 220.0)] {
            let input = self.graph.add(NodeKind::SolverInput { index }, egui::pos2(x, 20.0));
            if let Some(n) = self.graph.nodes.get_mut(&input) {
                n.context = AppContext::Obj;
                n.parent = Some(id);
            }
            if index == 0 {
                self.graph.set_display(input);
            }
        }
        self.status = "SOP Solver created: double-click it to edit its network (Prev_Frame is last step's result)".into();
    }

    // Every selected node: the multi-selection when it includes the current node,
    // otherwise just the current node.
    fn selected_nodes(&self) -> Vec<NodeId> {
        let Some(current) = self.selected else { return Vec::new() };
        if self.node_selection.contains(&current) {
            self.node_selection.iter().copied().filter(|id| self.graph.nodes.contains_key(id)).collect()
        } else {
            vec![current]
        }
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Add(kind, pos) => {
                let is_particles = matches!(
                    kind,
                    NodeKind::Add { .. }
                        | NodeKind::Particles { .. }
                        | NodeKind::PopNet { .. }
                                | NodeKind::Flip { .. }
                        | NodeKind::WhitewaterSolver(_)
                        | NodeKind::Cache { .. }
                );
                let id = self.graph.add(kind, pos);
                let parent = self.view_parent();
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.context = self.context;
                    n.parent = parent;
                }
                self.populate_new_node(id);
                self.selected = Some(id);
                if is_particles {
                    self.graph.set_display(id);
                    self.show_points = true;
                    self.status = "Particle/fluid output displayed; Points mode enabled".into();
                }
            }
            Action::AddBelow(kind, from) => {
                let pos = self
                    .graph
                    .nodes
                    .get(&from)
                    .map_or(egui::pos2(40.0, 40.0), |n| n.pos + egui::vec2(0.0, 80.0));
                let takes_input = kind.num_inputs() > 0;
                let is_particles = matches!(
                    kind,
                    NodeKind::Add { .. }
                        | NodeKind::Particles { .. }
                        | NodeKind::PopNet { .. }
                                | NodeKind::Flip { .. }
                        | NodeKind::WhitewaterSolver(_)
                        | NodeKind::Cache { .. }
                );
                let id = self.graph.add(kind, pos);
                let parent = self.view_parent();
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.context = self.context;
                    n.parent = parent;
                }
                self.populate_new_node(id);
                // A loop or compile block takes its input at its Begin.
                let target = match self.graph.nodes.get(&id).map(|n| &n.kind) {
                    Some(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. })) | Some(NodeKind::CompileEnd { begin: Some(b) }) => *b,
                    _ => id,
                };
                if takes_input {
                    if self.graph.nodes.get(&from).map_or(false, |n| n.context == self.context && n.parent == parent) {
                        self.graph.connect(from, target, 0);
                        if self.graph.is_display(from) {
                            self.graph.set_display(id);
                        }
                    }
                }
                self.selected = Some(id);
                if is_particles {
                    self.graph.set_display(id);
                    self.show_points = true;
                    self.status = "Particle/fluid output displayed; Points mode enabled".into();
                }
            }
            Action::Display(id) => match self.graph.nodes.get(&id).map(|n| &n.kind) {
                Some(NodeKind::Camera { .. }) => {
                    self.selected = Some(id);
                    self.use_camera_requested = true;
                }
                Some(kind) if kind.is_object() => {
                    if let Some(n) = self.graph.nodes.get_mut(&id) {
                        n.bypass = !n.bypass;
                    }
                }
                Some(_) => {
                    self.graph.set_display(id);
                }
                None => {}
            },
            Action::Enter(id) => self.enter_network(Some(id)),
            Action::Bypass(id) => {
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.bypass = !n.bypass;
                }
            }
            Action::Duplicate(id) => {
                if let Some(new) = self.graph.duplicate(id) {
                    self.selected = Some(new);
                }
            }
            Action::DuplicateMany(ids) => {
                let map = self.graph.duplicate_many(&ids);
                // The copies become the selection, so they can be dragged away together.
                self.node_selection = ids.iter().filter_map(|id| map.get(id).copied()).collect();
                self.selected = self.selected.and_then(|s| map.get(&s).copied()).or(self.node_selection.first().copied());
            }
            Action::Delete(id) => {
                self.graph.remove(id);
                if self.template == Some(id) {
                    self.template = None;
                }
                if self.selected.is_some_and(|s| !self.graph.nodes.contains_key(&s)) {
                    self.selected = None;
                }
                if self.active_camera == Some(id) {
                    self.active_camera = None;
                }
            }
        }
        self.dirty = true;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // File shortcuts work even while a text field (e.g. VEX code) has focus.
        // Save As is consumed first so Cmd+Shift+S isn't taken as Cmd+S.
        let (open, save, save_as) = ctx.input_mut(|i| {
            let save_as = i.consume_shortcut(&SHORTCUT_SAVE_AS);
            (i.consume_shortcut(&SHORTCUT_OPEN), i.consume_shortcut(&SHORTCUT_SAVE), save_as)
        });
        // Quitting closes the window, which asks about unsaved changes first.
        if ctx.input_mut(|i| i.consume_shortcut(&SHORTCUT_QUIT)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if save_as {
            self.save_scene_dialog();
        } else if save {
            self.save_scene();
        }
        if open {
            self.open_scene_dialog();
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let (undo, redo, dup, byp, frame, del, play, escape) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && ((i.modifiers.shift && i.key_pressed(egui::Key::Z)) || i.key_pressed(egui::Key::Y)),
                cmd && i.key_pressed(egui::Key::D),
                !cmd && i.key_pressed(egui::Key::B),
                !cmd && i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.animate = false;
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.simulation_rx.take().is_some() {
                self.simulation_cancel = None;
                self.cancelled_simulation_key = self.simulation_key.clone();
                self.status = "Cooking cancelled".into();
            }
            if self.sequence.take().is_some() {
                self.render_at = None;
                self.status = "Sequence cancelled (the current frame will finish)".into();
            }
        }
        if undo {
            self.do_undo();
        }
        if redo {
            self.do_redo();
        }
        if frame {
            self.frame_all();
        }
        if play {
            self.animate = !self.animate;
        }
        if let Some(sel) = self.selected {
            if ctx.input(|i| !i.modifiers.command && i.key_pressed(egui::Key::T)) {
                self.template = if self.template == Some(sel) { None } else { Some(sel) };
                self.dirty = true;
            }
        }
        let chosen = self.selected_nodes();
        if dup {
            match chosen.as_slice() {
                [] => {}
                [one] => self.apply(Action::Duplicate(*one)),
                many => self.apply(Action::DuplicateMany(many.to_vec())),
            }
        }
        if byp {
            for &id in &chosen {
                self.apply(Action::Bypass(id));
            }
        }
        if del && self.selected_vertices.is_empty()
            && self.selected_triangles.is_empty()
            && self.selected_edges.is_empty()
        {
            for &id in &chosen {
                self.apply(Action::Delete(id));
            }
            self.node_selection.clear();
        }
        if del
            && (!self.selected_vertices.is_empty()
                || !self.selected_triangles.is_empty()
                || !self.selected_edges.is_empty())
        {
            self.create_blast_from_selection();
        }
    }

    // Shows the node's input and selects its current group, ready for picking.
    fn start_group_pick(&mut self, id: NodeId, target: PickTarget) {
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let (group, support) = (node.group.clone(), node.kind.group_support());
        let mut input = self.graph.input_of(id, 0).map(|i| self.graph.cook(i, self.time, &mut HashMap::new())).unwrap_or_default();
        let members = |mask: Result<Vec<bool>, String>| -> Vec<usize> { mask.map(|m| m.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect()).unwrap_or_default() };
        let (domain, current) = match (target, &node.kind) {
            // Starts from the group the node makes now (before Combine and Invert), so
            // a bounding box or random group can be touched up by hand.
            (PickTarget::GroupCreate, NodeKind::GroupCreate(set)) => {
                let raw = groups::GroupCreateSettings { name: "pick".into(), combine: 0, invert: false, ..set.clone() };
                let current = match groups::group_create(&input, &raw) {
                    Ok(made) => made.point_attributes.get(&groups::key("pick", set.domain)).map(|v| v.iter().enumerate().filter(|(_, b)| **b > 0.5).map(|(i, _)| i).collect()).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                (set.domain, current)
            }
            // Materials go on primitives.
            (PickTarget::MaterialSlot(slot), NodeKind::MaterialAssign { slots }) => {
                let pattern = slots.get(slot).map_or("", |(g, _)| g.as_str()).trim();
                let current = if pattern.is_empty() {
                    Vec::new()
                } else {
                    let mask = groups::prim_mask(&input, &groups::NodeGroup { pattern: pattern.to_owned(), kind: 2 })
                        .or_else(|_| groups::prim_mask(&input, &groups::NodeGroup { pattern: pattern.to_owned(), kind: 0 }));
                    members(mask.map(|m| m.unwrap_or_default()))
                };
                (groups::GroupDomain::Prims, current)
            }
            _ => {
                let domain = match group.kind {
                    1 => groups::GroupDomain::Points,
                    2 => groups::GroupDomain::Prims,
                    // Nodes that work on primitives (scatter, groom roots, blast) start with primitives.
                    _ if group.is_empty() && support == GroupSupport::Native && !matches!(node.kind, NodeKind::Wrangle { .. }) => groups::GroupDomain::Prims,
                    _ => groups::guess_domain(&input, &group.pattern),
                };
                let current = if group.is_empty() { Vec::new() } else { members(groups::evaluate(&input, &group.pattern, domain)) };
                (domain, current)
            }
        };
        if let Some(object) = self.graph.enclosing_geometry(id) {
            input = self.graph.xform_at(object, self.time).apply(input);
        }
        self.clear_component_selection();
        let previous_mode = self.selection_mode;
        match domain {
            groups::GroupDomain::Points => {
                self.selection_mode = SelectionMode::Vertex;
                self.selected_vertices = current;
            }
            groups::GroupDomain::Prims => {
                self.selection_mode = SelectionMode::Triangle;
                self.selected_triangles = current;
            }
        }
        self.current_mesh = input.clone();
        self.mesh_revision += 1;
        self.group_pick = Some(GroupPick { node: id, target, domain, input, previous_mode });
        self.status = "Select in the viewport, then press Enter (or ✓) to set the group; Esc cancels".into();
    }

    // Switches a running pick between points and primitives.
    fn set_group_pick_domain(&mut self, domain: groups::GroupDomain) {
        if let Some(pick) = self.group_pick.as_mut().filter(|p| !matches!(p.target, PickTarget::MaterialSlot(_))) {
            pick.domain = domain;
            self.selection_mode = if domain == groups::GroupDomain::Points { SelectionMode::Vertex } else { SelectionMode::Triangle };
            self.clear_component_selection();
        }
    }

    fn finish_group_pick(&mut self, accept: bool) {
        let Some(pick) = self.group_pick.take() else { return };
        if accept {
            let selected = match pick.domain {
                groups::GroupDomain::Points => self.selected_vertices.clone(),
                groups::GroupDomain::Prims => self.selected_triangles.clone(),
            };
            let pattern = groups::describe(&pick.input, &selected, pick.domain);
            if let Some(node) = self.graph.nodes.get_mut(&pick.node) {
                match (pick.target, &mut node.kind) {
                    (PickTarget::GroupCreate, NodeKind::GroupCreate(set)) => {
                        set.domain = pick.domain;
                        set.method = 3;
                        self.status = if pattern.is_empty() { "Nothing selected: the group is empty".into() } else { format!("Group expression set to {pattern}") };
                        set.expression = pattern;
                    }
                    (PickTarget::MaterialSlot(slot), NodeKind::MaterialAssign { slots }) => {
                        if let Some((group, _)) = slots.get_mut(slot) {
                            self.status = if pattern.is_empty() { format!("Group {} cleared (nothing selected): its material goes on everything", slot + 1) } else { format!("Group {} set to {pattern}", slot + 1) };
                            *group = pattern;
                        }
                    }
                    _ => {
                        node.group.kind = if pick.domain == groups::GroupDomain::Points { 1 } else { 2 };
                        self.status = if pattern.is_empty() { "Group cleared (nothing selected): the node applies to everything".into() } else { format!("Group set to {pattern}") };
                        node.group.pattern = pattern;
                    }
                }
            }
        } else {
            self.status = "Group selection cancelled".into();
        }
        self.selection_mode = pick.previous_mode;
        self.clear_component_selection();
        self.dirty = true;
    }

    fn clear_component_selection(&mut self) {
        self.selected_vertices.clear();
        self.selected_triangles.clear();
        self.selected_edges.clear();
    }

    fn select_edge_loop(&mut self, seed: (usize, usize)) {
        let seed = (seed.0.min(seed.1), seed.0.max(seed.1));
        let mut incident: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for tri in &self.current_mesh.indices {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let edge = (a.min(b), a.max(b));
                incident.entry(a).or_default().push(edge);
                incident.entry(b).or_default().push(edge);
            }
        }
        for edges in incident.values_mut() {
            edges.sort_unstable();
            edges.dedup();
        }
        let mut edges = HashSet::from([seed]);
        let vertices = &self.current_mesh.vertices;
        let continue_from = |edge: (usize, usize), endpoint: usize, edges: &HashSet<(usize, usize)>| {
            let other = if edge.0 == endpoint { edge.1 } else { edge.0 };
            let direction = vertices[other].sub(vertices[endpoint]).normalize();
            incident.get(&endpoint)
                .into_iter()
                .flatten()
                .filter(|candidate| **candidate != edge && !edges.contains(candidate))
                .filter_map(|candidate| {
                    let next = if candidate.0 == endpoint { candidate.1 } else { candidate.0 };
                    let next_direction = vertices[next].sub(vertices[endpoint]).normalize();
                    let alignment = direction.dot(next_direction).abs();
                    (alignment > 0.35).then_some((alignment, *candidate))
                })
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(_, candidate)| candidate)
        };
        for &(endpoint, other) in &[(seed.0, seed.1), (seed.1, seed.0)] {
            let mut current = (endpoint, other);
            for _ in 0..2048 {
                let Some(next) = continue_from(
                    (current.0.min(current.1), current.0.max(current.1)),
                    current.1,
                    &edges,
                ) else {
                    break;
                };
                edges.insert(next);
                current = (current.1, if next.0 == current.1 { next.1 } else { next.0 });
            }
        }
        self.selected_edges = edges.into_iter().collect();
        self.selected_edges.sort_unstable();
    }

    fn select_all_components(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                self.selected_vertices = (0..self.current_mesh.vertices.len()).collect();
            }
            SelectionMode::Triangle => {
                self.selected_triangles = (0..self.current_mesh.indices.len()).collect();
            }
            SelectionMode::Edge => {
                let mut edges = HashSet::new();
                for tri in &self.current_mesh.indices {
                    let pairs = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
                    for (a, b) in pairs {
                        edges.insert(if a < b { (a, b) } else { (b, a) });
                    }
                }
                self.selected_edges = edges.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn invert_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                self.selected_vertices = (0..self.current_mesh.vertices.len())
                    .filter(|index| !selected.contains(index))
                    .collect();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                self.selected_triangles = (0..self.current_mesh.indices.len())
                    .filter(|index| !selected.contains(index))
                    .collect();
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                self.select_all_components();
                self.selected_edges.retain(|edge| !selected.contains(edge));
            }
        }
    }

    fn grow_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                let mut grown = selected.clone();
                for tri in &self.current_mesh.indices {
                    if tri.iter().any(|index| selected.contains(index)) {
                        grown.extend(tri);
                    }
                }
                self.selected_vertices = grown.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                let mut grown = selected.clone();
                for (index, tri) in self.current_mesh.indices.iter().enumerate() {
                    if selected.iter().any(|selected_index| {
                        self.current_mesh.indices.get(*selected_index).is_some_and(|other| {
                            tri.iter().any(|vertex| other.contains(vertex))
                        })
                    }) {
                        grown.insert(index);
                    }
                }
                let grown: Vec<usize> = grown.into_iter().collect();
                self.selected_triangles = poly::whole_polygons(&self.current_mesh, &grown);
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                let mut grown = selected.clone();
                for tri in &self.current_mesh.indices {
                    let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                        .map(|(a, b)| if a < b { (a, b) } else { (b, a) });
                    if edges.iter().any(|edge| selected.contains(edge)) {
                        grown.extend(edges);
                    }
                }
                self.selected_edges = grown.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn shrink_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                let mut retained = HashSet::new();
                for &vertex in &selected {
                    let incident: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .filter(|tri| tri.contains(&vertex))
                        .collect();
                    if !incident.is_empty()
                        && incident.iter().all(|tri| tri.iter().all(|index| selected.contains(index)))
                    {
                        retained.insert(vertex);
                    }
                }
                self.selected_vertices = retained.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                let mut retained = HashSet::new();
                for &triangle_index in &selected {
                    let Some(triangle) = self.current_mesh.indices.get(triangle_index) else {
                        continue;
                    };
                    let neighbors: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .enumerate()
                        .filter(|(index, other)| {
                            *index != triangle_index
                                && triangle.iter().filter(|vertex| other.contains(vertex)).count() >= 2
                        })
                        .map(|(index, _)| index)
                        .collect();
                    if neighbors.iter().all(|neighbor| selected.contains(neighbor)) {
                        retained.insert(triangle_index);
                    }
                }
                // Only polygons that keep all their triangles stay selected.
                let runs = poly::runs(&self.current_mesh);
                self.selected_triangles = runs.into_iter().filter(|run| run.clone().all(|t| retained.contains(&t))).flatten().collect();
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                let mut retained = HashSet::new();
                for &edge in &selected {
                    let touching: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .filter(|tri| tri.contains(&edge.0) && tri.contains(&edge.1))
                        .collect();
                    if touching.iter().all(|tri| {
                        [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                            .iter()
                            .map(|&(a, b)| if a < b { (a, b) } else { (b, a) })
                            .all(|candidate| selected.contains(&candidate))
                    }) {
                        retained.insert(edge);
                    }
                }
                self.selected_edges = retained.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn select_boundary_edges(&mut self) {
        self.selection_mode = SelectionMode::Edge;
        self.clear_component_selection();
        self.selected_edges = self
            .current_mesh
            .indices
            .iter()
            .flat_map(|tri| [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])])
            .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
            .filter(|edge| {
                self.current_mesh
                    .indices
                    .iter()
                    .filter(|tri| tri.contains(&edge.0) && tri.contains(&edge.1))
                    .count()
                    == 1
            })
            .collect();
        self.selected_edges.sort_unstable();
    }

    fn select_non_manifold_edges(&mut self) {
        let mut counts = HashMap::new();
        for tri in &self.current_mesh.indices {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let edge = if a < b { (a, b) } else { (b, a) };
                *counts.entry(edge).or_insert(0usize) += 1;
            }
        }
        self.selection_mode = SelectionMode::Edge;
        self.clear_component_selection();
        self.selected_edges = counts
            .into_iter()
            .filter_map(|(edge, count)| (count > 2).then_some(edge))
            .collect();
        self.selected_edges.sort_unstable();
    }

    fn select_isolated_vertices(&mut self) {
        let mut referenced: HashSet<usize> = HashSet::new();
        for tri in &self.current_mesh.indices {
            referenced.extend(tri);
        }
        self.selection_mode = SelectionMode::Vertex;
        self.clear_component_selection();
        self.selected_vertices = (0..self.current_mesh.vertices.len())
            .filter(|index| !referenced.contains(index))
            .collect();
    }

    fn select_connected_components(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let mut connected: HashSet<usize> = self.selected_vertices.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for tri in &self.current_mesh.indices {
                        if tri.iter().any(|vertex| connected.contains(vertex)) {
                            changed |= tri.iter().any(|vertex| connected.insert(*vertex));
                        }
                    }
                }
                self.selected_vertices = connected.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let mut connected: HashSet<usize> = self.selected_triangles.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for index in 0..self.current_mesh.indices.len() {
                        if connected.contains(&index) {
                            continue;
                        }
                        let triangle = self.current_mesh.indices[index];
                        if connected.iter().any(|selected| {
                            self.current_mesh.indices.get(*selected).is_some_and(|other| {
                                triangle.iter().filter(|vertex| other.contains(vertex)).count() >= 2
                            })
                        }) {
                            changed |= connected.insert(index);
                        }
                    }
                }
                self.selected_triangles = connected.into_iter().collect();
                self.selected_triangles.sort_unstable();
            }
            SelectionMode::Edge => {
                let mut connected: HashSet<(usize, usize)> =
                    self.selected_edges.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for tri in &self.current_mesh.indices {
                        let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                            .map(|(a, b)| if a < b { (a, b) } else { (b, a) });
                        if edges.iter().any(|edge| connected.contains(edge)) {
                            changed |= edges.iter().any(|edge| connected.insert(*edge));
                        }
                    }
                }
                self.selected_edges = connected.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    // ---------------- Parameter panel ----------------
    // Text typed into a parameter field: an expression makes the parameter follow it
    // (an expression channel), a plain number replaces an expression. The parameter
    // is the one whose value the typing changed.
    fn apply_typed(&mut self, (id, params, xform): (NodeId, Vec<f32>, ObjectXform), typed: Typed) {
        // Delete Channel: the node's animated parameters showing the field's value.
        if let Typed::Delete(value) = typed {
            let matching: Vec<ChannelId> = self
                .graph
                .node_channels(id)
                .map(|(c, _)| *c)
                .filter(|c| self.graph.channel_value(*c).is_some_and(|v| (v - value).abs() <= 1e-4 * value.abs().max(1.0)))
                .collect();
            if matching.is_empty() {
                self.status = "That parameter has no channel (expression or keys) to delete".into();
            } else {
                for channel in &matching {
                    self.graph.channels.remove(channel);
                }
                self.status = format!("Deleted {} channel{}", matching.len(), if matching.len() == 1 { "" } else { "s" });
                self.dirty = true;
            }
            return;
        }
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let now = node.kind.params();
        let mut targets: Vec<ChannelTarget> = (0..params.len().min(now.len())).filter(|&i| params[i] != now[i]).map(ChannelTarget::Param).collect();
        targets.extend((0..9).filter(|&i| xform_component(&xform, i) != xform_component(&node.xform, i)).map(ChannelTarget::Xform));
        if targets.is_empty() {
            if let Typed::Error(error) = typed {
                self.status = format!("Expression error: {error}");
            }
            return;
        }
        for target in targets {
            let channel_id = ChannelId { node: id, target };
            match &typed {
                Typed::Expression(text) => {
                    self.graph.channels.entry(channel_id).or_default().expression = Some(text.clone());
                    self.status = format!("The parameter follows {text}");
                }
                Typed::Number => {
                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                        channel.expression = None;
                        if channel.is_empty() {
                            self.graph.channels.remove(&channel_id);
                        }
                    }
                }
                Typed::Error(_) | Typed::Delete(_) => {}
            }
        }
        self.dirty = true;
    }

    // A Subnet's promoted parameters (adding and removing them) and its asset: saving it
    // to the library, and updating to the library's newer version.
    fn subnet_ui(&mut self, ui: &mut egui::Ui, id: NodeId) -> bool {
        let Some(NodeKind::Subnet { spec, .. }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else { return false };
        let mut changed = false;
        ui.separator();
        ui.strong("Promoted parameters");
        let mut remove = None;
        for (k, p) in spec.params.iter().enumerate() {
            let target = self.graph.nodes.get(&p.node).map_or_else(|| "(deleted node)".to_owned(), |n| format!("{} · {}", n.title(p.node), n.kind.param_names().get(p.index).cloned().unwrap_or_else(|| format!("#{}", p.index))));
            ui.horizontal(|ui| {
                ui.label(p.label.as_str());
                ui.weak(format!("→ {target}"));
                if ui.small_button("✕").on_hover_text("Stop promoting it (the node keeps its own value)").clicked() {
                    remove = Some(k);
                }
            });
        }
        // Nodes inside with numeric parameters, and the chosen one's parameters.
        let candidates: Vec<(NodeId, String, Vec<String>, Vec<f32>)> = self
            .graph
            .descendants(id)
            .into_iter()
            .filter_map(|n| {
                let node = self.graph.nodes.get(&n)?;
                let params = node.kind.params();
                (!params.is_empty() && !node.kind.is_container()).then(|| {
                    let names = node.kind.param_names();
                    (n, node.title(n), (0..params.len()).map(|i| names.get(i).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| format!("#{i}"))).collect(), params)
                })
            })
            .collect();
        if candidates.is_empty() {
            ui.small("Nothing inside has parameters to promote yet.");
        } else {
            let (mut node, mut index, mut label) = self.subnet_promote.clone();
            if !node.is_some_and(|n| candidates.iter().any(|c| c.0 == n)) {
                (node, index) = (Some(candidates[0].0), 0);
            }
            let chosen = candidates.iter().find(|c| Some(c.0) == node).expect("chosen above");
            index = index.min(chosen.2.len() - 1);
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt(("promote_node", id)).selected_text(chosen.1.as_str()).show_ui(ui, |ui| {
                    for (n, title, ..) in &candidates {
                        if ui.selectable_value(&mut node, Some(*n), title.as_str()).changed() {
                            index = 0;
                        }
                    }
                });
                let chosen = candidates.iter().find(|c| Some(c.0) == node).expect("one of them");
                index = index.min(chosen.2.len() - 1);
                egui::ComboBox::from_id_salt(("promote_param", id)).selected_text(chosen.2[index].as_str()).show_ui(ui, |ui| {
                    for (i, name) in chosen.2.iter().enumerate() {
                        ui.selectable_value(&mut index, i, name.as_str());
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut label).hint_text(chosen.2[index].as_str()).desired_width(100.0));
                if ui.button("Promote").on_hover_text("Show this parameter on the Subnet, as a slider that sets it").clicked() {
                    let value = chosen.3[index];
                    let label = if label.trim().is_empty() { chosen.2[index].clone() } else { label.trim().to_owned() };
                    let (min, max) = if value >= 0.0 { (0.0, (value * 2.0).max(1.0)) } else { (value * 2.0, (-value * 2.0).max(1.0)) };
                    if let Some(NodeKind::Subnet { values, spec }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                        spec.params.push(assets::Promoted { label, node: chosen.0, index, min, max });
                        values.push(value);
                    }
                    self.subnet_promote.2.clear();
                    changed = true;
                    return;
                }
            });
            if !changed {
                self.subnet_promote = (node, index, label);
            }
        }
        if let Some(k) = remove {
            if let Some(NodeKind::Subnet { values, spec }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                spec.params.remove(k);
                if k < values.len() {
                    values.remove(k);
                }
            }
            changed = true;
        }

        ui.separator();
        ui.strong("Asset");
        let newer = spec.asset.as_ref().and_then(|(name, version)| self.asset_library.iter().find(|e| &e.name == name && e.version > *version).cloned());
        if let Some(entry) = &newer {
            let older = self.graph.asset_copies(&entry.name, entry.version).len();
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), format!("The library has version {}", entry.version));
                if ui.button("Update").on_hover_text("Replace the network inside with the library's version. Sliders keep their values where the new version has them.").clicked() {
                    let result = entry.text().and_then(|text| self.graph.fill_from_asset(id, &text));
                    self.status = match result {
                        Ok(()) => format!("Updated to {} version {}", entry.name, entry.version),
                        Err(e) => format!("Couldn't update: {e}"),
                    };
                    changed = true;
                }
                if older > 1 && ui.button(format!("Update all {older} copies")).on_hover_text("Update every copy of this asset in the scene that's on an older version, each keeping its sliders' values").clicked() {
                    self.status = match entry.text().and_then(|text| self.graph.update_asset_copies(&entry.name, entry.version, &text)) {
                        Ok(count) => format!("Updated {count} cop{} of {} to version {}", if count == 1 { "y" } else { "ies" }, entry.name, entry.version),
                        Err(e) => format!("Couldn't update: {e}"),
                    };
                    changed = true;
                }
            });
        }
        let Some(dir) = self.asset_dir.clone() else {
            ui.small("There's no assets folder on this computer.");
            return changed;
        };
        let (mut name, mut description) = match &self.asset_save {
            (Some(for_id), name, description) if *for_id == id => (name.clone(), description.clone()),
            _ => (String::new(), String::new()),
        };
        if name.is_empty() {
            name = spec.asset.as_ref().map_or_else(|| self.graph.nodes[&id].title(id), |(n, _)| n.clone());
        }
        if description.is_empty() {
            description = spec.description.clone();
        }
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut name);
        });
        ui.add(egui::TextEdit::multiline(&mut description).hint_text("What it does (shown in the TAB menu)").desired_rows(2));
        let in_library = self.asset_library.iter().find(|e| e.name == name.trim()).map(|e| e.version);
        let label = match in_library {
            Some(v) => format!("Save as version {}", v + 1),
            None => "Save as Asset".to_owned(),
        };
        ui.horizontal(|ui| {
            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new(label)).on_hover_text("Write it to the assets folder: it appears in the TAB menu under Assets, wherever geometry nodes can go").clicked() {
                let name = name.trim().to_owned();
                let version = in_library.map_or(1, |v| v + 1).max(spec.asset.as_ref().filter(|(n, _)| *n == name).map_or(1, |(_, v)| v + 1));
                // A built-in asset saved again goes to the assets folder, replacing it.
                let path = self.asset_library.iter().find(|e| e.name == name).and_then(|e| e.path.clone()).unwrap_or_else(|| dir.join(assets::file_name(&name)));
                let text = self.graph.asset_text(id, &name, version, description.trim());
                let written = std::fs::create_dir_all(&dir).and_then(|_| write_file_atomically(&path, &text));
                self.status = match written {
                    Ok(()) => {
                        if let Some(NodeKind::Subnet { spec, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                            spec.asset = Some((name.clone(), version));
                            spec.description = description.trim().to_owned();
                        }
                        self.asset_library = assets::library(Some(&dir));
                        changed = true;
                        let others = self.graph.nodes.iter().filter(|(other, n)| **other != id && matches!(&n.kind, NodeKind::Subnet { spec: assets::Spec { asset: Some((n2, v)), .. }, .. } if *n2 == name && *v < version)).count();
                        let note = if others > 0 { format!(". {others} other cop{} in this scene can be updated from {} panel", if others == 1 { "y" } else { "ies" }, if others == 1 { "its" } else { "their" }) } else { String::new() };
                        format!("Saved {name} version {version} to {}{note}", path.display())
                    }
                    Err(e) => format!("Couldn't save the asset: {e}"),
                };
            }
            if ui.button("Show the assets folder").clicked() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = platform::open(&dir.to_string_lossy());
            }
            if ui.small_button("⟳").on_hover_text("Read the assets folder again (after copying assets into it)").clicked() {
                self.asset_library = assets::library(Some(&dir));
            }
        });
        self.asset_save = (Some(id), name, description);
        changed
    }

    fn params_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(id) = self.selected else {
            ui.label("Select a node to edit its parameters.");
            return false;
        };
        let is_display = self.graph.is_display(id);
        let geometry_objects = self.graph.geometry_ids();
        let picked_prim = self.stage_selected_prim.clone();
        let viewport_view = self.viewport_eye_target();
        let pop_display_missing = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::PopNet { .. })) && self.graph.display_of(id).is_none();
        let solver_display_is_input = self
            .graph
            .display_of(id)
            .and_then(|d| self.graph.nodes.get(&d))
            .is_some_and(|n| matches!(n.kind, NodeKind::SolverInput { .. }));
        let camera_ids = self.camera_ids();
        let mut material_ids: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.is_material())
            .filter(|(_, n)| n.parent.is_none())
            .map(|(mid, n)| (*mid, n.title(*mid)))
            .collect();
        material_ids.sort_unstable();
        // An Attribute VOP's generated code and its Parameter VOPs' defaults.
        let vop_code = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. }) | Some(NodeKind::Pop(pop::PopNode { op: pop::VOP, .. })))
            .then(|| self.graph.vop_code(id));
        let vop_parameters: Vec<(String, [f32; 3])> = self
            .graph
            .children(id)
            .into_iter()
            .filter_map(|child| match &self.graph.nodes.get(&child)?.kind {
                NodeKind::Vop(v) if v.op == vop::PARAMETER => Some((v.name.clone(), v.value)),
                _ => None,
            })
            .collect();
        let switch_inputs = self.graph.links.iter().filter(|l| l.to == id).map(|l| l.slot + 1).max().unwrap_or(0);
        // Joint names of the input skeleton, for KineFX nodes' joint menus.
        let joint_names: Vec<String> = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::RigPose(_) | NodeKind::TwoBoneIk(_)) => {
                let input = self.graph.cook_fresh(self.graph.input_of(id, 0), self.time);
                kinefx::Skeleton::from_mesh(&input).map(|s| s.names).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        // Group names on the node's input, for the Group field's menu (filled in on request).
        let group_choices = self.group_choices.as_ref().filter(|(for_id, ..)| *for_id == id).map(|(_, p, q)| (p.clone(), q.clone()));
        let focus_rename = std::mem::take(&mut self.focus_rename);
        // TOP nodes: what they can fetch or wedge, their items and the last cook's results.
        let (fetch_targets, wedge_targets, top_preview) = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Top(_)) => {
                let mut fetch: Vec<(NodeId, String)> = self
                    .graph
                    .nodes
                    .iter()
                    .filter(|(_, n)| matches!(n.kind, NodeKind::MoonRayRender { .. } | NodeKind::FileExport { .. } | NodeKind::Cache { .. }))
                    .map(|(nid, n)| (*nid, n.title(*nid)))
                    .collect();
                fetch.sort_unstable();
                let mut wedge: Vec<(NodeId, String, Vec<(usize, String)>)> = self
                    .graph
                    .nodes
                    .iter()
                    .filter(|(_, n)| !matches!(n.kind, NodeKind::Top(_) | NodeKind::TopNetwork))
                    .filter_map(|(nid, n)| {
                        let params: Vec<(usize, String)> = n.kind.animatable().into_iter().filter_map(|(t, name)| match t {
                            ChannelTarget::Param(i) => Some((i, name)),
                            _ => None,
                        }).collect();
                        (!params.is_empty()).then(|| (*nid, n.title(*nid), params))
                    })
                    .collect();
                wedge.sort_by_key(|w| w.0);
                (fetch, wedge, Some(self.graph.top_items(id)))
            }
            _ => (Vec::new(), Vec::new(), None),
        };
        let top_states = self.top_results.get(&id).cloned().unwrap_or_default();
        // Object Merge: what it can merge, by object: each object's display output and
        // the nodes inside it (its own network too, except what's downstream of the merge).
        let merge_sources: Vec<(NodeId, String, Vec<(NodeId, String)>)> = if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::ObjectMerge { .. })) {
            let downstream = self.graph.downstream(id);
            let mut objects: Vec<(NodeId, String, Vec<(NodeId, String)>)> = self
                .graph
                .nodes
                .iter()
                .filter(|(_, n)| n.context == AppContext::Obj && matches!(n.kind, NodeKind::Geometry { .. }))
                .map(|(oid, n)| (*oid, n.title(*oid), Vec::new()))
                .collect();
            for (nid, n) in &self.graph.nodes {
                if n.context != AppContext::Obj || matches!(n.kind, NodeKind::Geometry { .. }) || downstream.contains(nid) {
                    continue;
                }
                let Some(object) = self.graph.enclosing_geometry(*nid) else { continue };
                if let Some(entry) = objects.iter_mut().find(|(o, ..)| *o == object) {
                    entry.2.push((*nid, self.graph.path_in_object(*nid)));
                }
            }
            objects.sort_by(|a, b| a.1.cmp(&b.1));
            for (.., nodes) in &mut objects {
                nodes.sort_by(|a, b| a.1.cmp(&b.1));
            }
            objects
        } else {
            Vec::new()
        };
        let merge_own_object = self.graph.enclosing_geometry(id);
        // Match Size: whether input 2 gives the target box.
        let has_reference = self.graph.input_of(id, 1).is_some();
        let loop_begin_name = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. }) | NodeKind::CompileEnd { begin: Some(b) }) => self.graph.nodes.get(b).map(|n| n.title(*b)),
            Some(NodeKind::ForEachBegin) | Some(NodeKind::CompileBegin) => self
                .graph
                .nodes
                .iter()
                .find(|(_, n)| matches!(n.kind, NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. }) | NodeKind::CompileEnd { begin: Some(b) } if b == id))
                .map(|(e, n)| n.title(*e)),
            _ => None,
        };
        let compiled_loop = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. })) if self.graph.in_compile_block(*b));
        let top_busy = self.top_run.is_some();
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            return false;
        };
        let is_object = node.kind.is_object();
        let title = node.title(id);
        ui.heading(match &node.kind {
            NodeKind::Vop(v) => format!("{} · {title}", v.def().label),
            NodeKind::ShaderNode(n) => format!("{} · {title}", n.def().label),
            _ if !node.name.trim().is_empty() => format!("{title} · {}", node.type_label()),
            _ => title,
        });
        let mut c = false;
        // Renaming: empty goes back to the generated name.
        ui.horizontal(|ui| {
            ui.label("Name");
            let field = ui.add(egui::TextEdit::singleline(&mut node.name).hint_text(node.kind.label(id)).desired_width(160.0));
            if focus_rename {
                field.request_focus();
            }
            c |= field.changed();
            if ui.button("ℹ").on_hover_text("Node Info: counts, bounds, attributes and groups (or hold the middle button on the node)").clicked() {
                self.inspect_node = Some(id);
            }
        });
        ui.horizontal(|ui| {
            if is_object {
                let mut active = !node.bypass;
                if ui.checkbox(&mut active, "Active").changed() {
                    node.bypass = !active;
                    c = true;
                }
            } else {
                c |= ui.checkbox(&mut node.bypass, "Bypass").changed();
                ui.label(if is_display { "● display" } else { "" });
            }
        });
        let support = node.kind.group_support();
        if support != GroupSupport::None {
            ui.horizontal(|ui| {
                ui.label(if support == GroupSupport::Pin { "Pin group" } else { "Group" });
                c |= ui
                    .add(egui::TextEdit::singleline(&mut node.group.pattern).hint_text("everything").desired_width(150.0))
                    .on_hover_text("Names (a b), wildcards (side*), numbers (0-10 20-40:2), expressions (@P.y>0), ^name to exclude, !name for everything else")
                    .changed();
                let menu = ui.menu_button("▾", |ui| {
                    match &group_choices {
                        None => {
                            ui.weak("Reading groups…");
                        }
                        Some((points, prims)) if points.is_empty() && prims.is_empty() => {
                            ui.weak("The input has no groups (make one with Group Create)");
                        }
                        Some((points, prims)) => {
                            for (list, label) in [(points, "Point groups"), (prims, "Primitive groups")] {
                                if list.is_empty() {
                                    continue;
                                }
                                ui.strong(label);
                                for name in list {
                                    if ui.button(name).clicked() {
                                        node.group.pattern = name.clone();
                                        c = true;
                                        ui.close();
                                    }
                                }
                            }
                        }
                    }
                });
                if menu.inner.is_some() {
                    self.group_choices_request = Some(id);
                }
                let picking = self.group_pick.as_ref().is_some_and(|p| p.node == id && p.target == PickTarget::NodeGroup);
                if !picking && ui.button("⬚ Select").on_hover_text("Pick the group's points or primitives in the viewport").clicked() {
                    self.group_pick_request = Some((id, PickTarget::NodeGroup));
                }
                egui::ComboBox::from_id_salt("group_type").selected_text(groups::GROUP_TYPES[node.group.kind.min(2) as usize]).width(110.0).show_ui(ui, |ui| {
                    for (value, label) in groups::GROUP_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut node.group.kind, value as u8, *label).changed();
                    }
                });
            });
            if let Some(pick) = self.group_pick.as_ref().filter(|p| p.node == id && p.target == PickTarget::NodeGroup) {
                let picked = match pick.domain {
                    groups::GroupDomain::Points => self.selected_vertices.len(),
                    groups::GroupDomain::Prims => self.selected_triangles.len(),
                };
                let domain = pick.domain;
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 210, 90), format!("Selecting: {picked}"));
                    for (d, label) in [(groups::GroupDomain::Points, "Points"), (groups::GroupDomain::Prims, "Primitives")] {
                        if ui.selectable_label(domain == d, label).clicked() && domain != d {
                            self.group_pick_domain = Some(d);
                        }
                    }
                    if ui.button("✓ Accept").clicked() {
                        self.group_pick_finish = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        self.group_pick_finish = Some(false);
                    }
                });
            }
        }
        ui.separator();

        match &mut node.kind {
            NodeKind::Geometry { material } => {
                ui.label("Object container. Its SOP network defines the shape rendered in the scene.");
                ui.strong("Transform");
                c |= vec3_ui(ui, "Translate", &mut node.xform.t, 0.05);
                c |= vec3_ui(ui, "Rotate°", &mut node.xform.r, 0.5);
                c |= vec3_ui(ui, "Scale", &mut node.xform.s, 0.02);
                ui.small("Or drag the gizmo in the viewport (W move, E rotate, R scale).");
                let selected_name = material
                    .and_then(|m| material_ids.iter().find(|(mid, _)| *mid == m))
                    .map_or("default (grey)".to_string(), |(_, name)| name.clone());
                ui.horizontal(|ui| {
                    ui.label("Material");
                    egui::ComboBox::from_id_salt("geometry_material")
                        .selected_text(selected_name)
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(material, None, "default (grey)").changed();
                            for (mid, name) in &material_ids {
                                c |= ui.selectable_value(material, Some(*mid), name).changed();
                            }
                        });
                });
                ui.small("Create materials in the MAT context.");
                if ui.button("Enter geometry network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("Double-click the node to enter; press U in the network to go back up.");
            }
            NodeKind::CopNetwork => {
                ui.small("A compositing network: double-click to build it from COP nodes. The viewport shows the display node's image while you're in IMG.");
            }
            NodeKind::TopNetwork => {
                ui.small("A task graph: double-click to build it from TOP nodes, then cook the last node to run its work items.");
            }
            NodeKind::PopNet { substeps } => {
                ui.heading("POP Network");
                if pop_display_missing {
                    ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "⚠ Nothing inside has the display flag, so there are no particles. Double-click to enter it and set the display flag on the last node.");
                }
                c |= pfield(ui, pslider(substeps, 1..=16).text("Substeps")).changed();
                ui.small("A particle simulation built from POP nodes: double-click it to edit. A new one has a POP Source in the POP Solver's third input (post-solve, as in Houdini: particles are born at the source at the end of each step), with Gravity Force after it. Inputs 0-3 are geometry for POP Sources to birth from (input 0 by default), for POP Collide to collide with, and for POP Wrangles to read (point(1, ...) reads input 0).");
                ui.small("The output is points with v, age, life and id, plus what the POPs add (Cd, groups...): ready for Copy to Points, Particle Fluid Surface, Loop Particles or a sprite sheet.");
            }
            NodeKind::Pop(pop_node) => {
                ui.heading(pop_node.def().label);
                ui.small(pop_node.def().help);
                let op = pop_node.op;
                if op == pop::WRANGLE {
                    if let Some(code) = pop_node.texts.get_mut(0) {
                        c |= vex_editor_ui(ui, code, 2, "particles", &mut node.channels, "v@force += set(0, 2, 0);");
                    }
                } else {
                    let labels = pop_node.def().texts;
                    let rule = pop_node.v("rule").round() as usize;
                    for (i, ((label, _), text)) in labels.iter().zip(pop_node.texts.iter_mut()).enumerate() {
                        // The expression only matters for the Expression rule.
                        let is_expression = (op == pop::GROUP && i == 1) || op == pop::KILL;
                        let expression_rule = (op == pop::GROUP && rule == 3) || (op == pop::KILL && rule == 0);
                        if is_expression && !expression_rule {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            let edit = egui::TextEdit::singleline(text).desired_width(220.0);
                            c |= ui.add(if is_expression { edit.font(egui::TextStyle::Monospace) } else { edit }).changed();
                        });
                    }
                }
                for (param, slot) in pop_node.param_mut() {
                    if let Some(choices) = pop::choices(op, param.key) {
                        let mut value = (slot[0].round().max(0.0) as usize).min(choices.len() - 1);
                        egui::ComboBox::from_label(param.label).selected_text(choices[value]).show_ui(ui, |ui| {
                            for (i, label) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut value, i, *label).changed();
                            }
                        });
                        slot[0] = value as f32;
                        continue;
                    }
                    match param.key {
                        "input" | "seed" => {
                            let mut whole = slot[0].round().max(0.0) as usize;
                            let top = if param.key == "input" { 3 } else { 10_000 };
                            c |= pfield(ui, pslider(&mut whole, 0..=top).text(param.label)).changed();
                            slot[0] = whole as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == pop::VOP {
                    ui.small("Double-click to edit its VOP network. Bind reads the particles' attributes (P, v, force, age, Cd...) and Export writes them back.");
                    match &vop_code {
                        Some(Ok(code)) => {
                            for (name, default) in &vop_parameters {
                                node.channels.entry(name.clone()).or_insert(*default);
                            }
                            c |= channel_params_ui(ui, code, &mut node.channels);
                            egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                                ui.label(egui::RichText::new(code.as_str()).monospace().small());
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                        }
                        None => {}
                    }
                }
            }
            NodeKind::Cop(cop) => {
                ui.small(cop.def().help);
                let labels = cop.def().texts;
                for (label, text) in labels.iter().zip(cop.texts.iter_mut()) {
                    ui.horizontal(|ui| {
                        ui.label(*label);
                        c |= ui.add(egui::TextEdit::singleline(text).desired_width(200.0)).changed();
                        if ui.button("…").clicked() {
                            let dialog = FileDialog::new().add_filter("Images", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff", "tga", "bmp"]);
                            let chosen = if cop.op == cops::OUTPUT { dialog.set_file_name("comp.$F4.exr").save_file() } else { dialog.pick_file() };
                            if let Some(path) = chosen {
                                *text = path.to_string_lossy().into_owned();
                                c = true;
                            }
                        }
                    });
                }
                let op = cop.op;
                for (param, slot) in cop.param_mut() {
                    match param.key {
                        "mode" => {
                            let mut mode = slot[0].round() as usize;
                            egui::ComboBox::from_label("Mode").selected_text(cops::BLEND_MODES[mode.min(9)]).show_ui(ui, |ui| {
                                for (value, label) in cops::BLEND_MODES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut mode, value, *label).changed();
                                }
                            });
                            slot[0] = mode as f32;
                        }
                        "type" if op == 3 => {
                            let mut kind = slot[0].round() as usize;
                            ui.horizontal(|ui| {
                                for (value, label) in cops::RAMP_TYPES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut kind, value, *label).changed();
                                }
                            });
                            slot[0] = kind as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == cops::OUTPUT {
                    ui.horizontal(|ui| {
                        if ui.button("💾 Write current frame").clicked() {
                            self.cop_write_request = Some((id, false));
                        }
                        if ui.button("Write timeline range").on_hover_text("Put $F or $F4 in the file name for one file per frame").clicked() {
                            self.cop_write_request = Some((id, true));
                        }
                    });
                }
            }
            NodeKind::Top(top) => {
                ui.small(top.def().help);
                let op = top.op;
                // Node pickers: what to fetch, or which node's parameter to wedge.
                if op == tops::FETCH {
                    let current = top.node();
                    let shown = current.and_then(|n| fetch_targets.iter().find(|(t, _)| *t == n)).map_or("(choose)".to_owned(), |(_, name)| name.clone());
                    egui::ComboBox::from_label("Fetch").selected_text(shown).show_ui(ui, |ui| {
                        for (target, name) in &fetch_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if fetch_targets.is_empty() {
                        ui.small("Add a MoonRay render node (OUT), or a File Export or Cache node (in a geometry network), to fetch.");
                    }
                }
                if op == tops::WEDGE {
                    let current = top.node();
                    let shown = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)).map_or("(none: attribute only)".to_owned(), |(_, name, _)| name.clone());
                    egui::ComboBox::from_label("Node").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "(none: attribute only)").clicked() {
                            top.set("node", -1.0);
                            c = true;
                        }
                        for (target, name, _) in &wedge_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if let Some((_, _, params)) = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)) {
                        let mut param = top.v("param") as usize;
                        let shown = params.iter().find(|(i, _)| *i == param).map_or("(choose)".to_owned(), |(_, n)| n.clone());
                        egui::ComboBox::from_label("Parameter").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                            for (i, name) in params {
                                c |= ui.selectable_value(&mut param, *i, name).changed();
                            }
                        });
                        top.set("param", param as f32);
                    }
                }
                let labels = top.def().texts;
                for (label, text) in labels.iter().zip(top.texts.iter_mut()) {
                    ui.label(*label);
                    c |= ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY)).changed();
                }
                if matches!(op, tops::FETCH | tops::COMMAND) {
                    ui.small("Tokens: $F / $F4 frame, @index, @wedgeindex, @<wedge attribute>, {input} (the item's input files).");
                }
                let def = top.def();
                for (i, param) in def.params.iter().enumerate() {
                    if param.key == "node" || param.key == "param" {
                        continue;
                    }
                    let mut v = top.values[i];
                    if let shading::Kind::Float(min, max) = param.kind {
                        if matches!(param.key, "count" | "start" | "end" | "step") && op != tops::WEDGE || param.key == "count" || param.key == "step" {
                            let mut whole = v.round() as i64;
                            c |= pfield(ui, pdrag(&mut whole).range(min as i64..=max as i64).prefix(format!("{}: ", param.label))).changed();
                            v = whole as f32;
                        } else {
                            c |= pfield(ui, pdrag(&mut v).speed(0.01).range(min..=max).prefix(format!("{}: ", param.label))).changed();
                        }
                    }
                    top.values[i] = v;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(!top_busy, egui::Button::new("▶ Cook")).on_hover_text("Runs this node's work items (and its inputs' first)").clicked() {
                        self.top_cook_request = Some(id);
                    }
                    if ui.add_enabled(top_busy, egui::Button::new("■ Cancel")).clicked() {
                        self.top_cancel_request = true;
                    }
                });
                match &top_preview {
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 120, 100), e);
                    }
                    Some(Ok(items)) => {
                        let done = top_states.iter().filter(|s| s.0 == tops::State::Done).count();
                        let failed = top_states.iter().filter(|s| s.0 == tops::State::Failed).count();
                        ui.label(format!("{} work items{}", items.len(), if top_states.is_empty() { String::new() } else { format!(" · {done} done · {failed} failed") }));
                        egui::ScrollArea::vertical().max_height(260.0).id_salt(("top_items", id)).show(ui, |ui| {
                            for (i, item) in items.iter().take(500).enumerate() {
                                let (state, log) = top_states.get(i).cloned().unwrap_or((tops::State::Waiting, String::new()));
                                let colour = top_state_colour(state);
                                let mut line = format!("#{} · frame {}", item.index, item.frame);
                                for (name, value) in &item.attrs {
                                    line += &format!(" · {name} {value}");
                                }
                                ui.horizontal(|ui| {
                                    ui.colored_label(colour, "●");
                                    ui.label(line);
                                })
                                .response
                                .on_hover_text(match &item.job {
                                    tops::Job::Shell(cmd) => format!("{cmd}\n{log}"),
                                    _ if !log.is_empty() => log.clone(),
                                    _ => item.output.clone(),
                                });
                                if !item.output.is_empty() {
                                    ui.small(&item.output);
                                }
                            }
                        });
                    }
                    None => {}
                }
            }
            NodeKind::Shader(values) => {
                ui.small(match values.table {
                    shading::Table::Principled => "Physically based surface (MoonRay DwaBaseMaterial). Assign it from a Geometry node. Opacity, and Use point alpha (the geometry's Alpha), also show in the viewport and sprite sheets.",
                    shading::Table::Toon => "Cel-shaded surface with banded shadows, a toon highlight and outlines, in the viewport and sprite sheets as in MoonRay (DwaToonMaterial; outlines are real-time only).",
                    shading::Table::Hair => "For hair and fur curves from Hair Generate (HairMaterial_v3).",
                    shading::Table::Pyro => "For Pyro and other fog volumes: smoke colour and density, fire brightness and tint.",
                });
                c |= shader_values_ui(ui, values);
            }
            NodeKind::MaterialBuilder => {
                ui.small("Build a material from texture nodes: double-click to enter, then wire Image Texture, Noise, Mix… into the Surface Output's inputs.");
            }
            NodeKind::ShaderNode(node) => {
                ui.small(node.def().help);
                if node.op == shading::SURFACE_OUTPUT {
                    ui.small("Connected inputs replace the values below.");
                    c |= shader_values_ui(ui, &mut node.surface);
                } else {
                    let labels = node.def().texts;
                    for (label, text) in labels.iter().zip(node.texts.iter_mut()) {
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            c |= ui.text_edit_singleline(text).changed();
                            if *label == "File" && ui.button("…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "exr", "tif", "tiff", "hdr", "tx"]).pick_file() {
                                    *text = path.to_string_lossy().into_owned();
                                    c = true;
                                }
                            }
                        });
                    }
                    for (param, slot) in node.param_mut() {
                        c |= shader_param_ui(ui, param, slot);
                    }
                }
            }
            NodeKind::Material { kind, color, roughness, value } => {
                ui.label("MoonRay material. Assign it from a Geometry node.");
                ui.horizontal(|ui| {
                    for (index, name) in MATERIAL_NAMES.iter().enumerate() {
                        c |= ui.selectable_value(kind, index, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                match *kind {
                    0 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= pfield(ui, pslider(value, 0.0..=1.0).text("Specular")).changed();
                    }
                    1 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                    }
                    2 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= pfield(ui, pslider(value, 1.0..=3.0).text("IOR")).changed();
                    }
                    _ => {
                        c |= pfield(ui, pslider(value, 0.0..=50.0).logarithmic(true).text("Intensity")).changed();
                    }
                }
            }
            NodeKind::Add { count, spacing, layout, points, delete_geometry } => {
                ui.small("Adds points (to the input, if one is connected): at the positions below, and/or a line or grid of them.");
                c |= ui.checkbox(delete_geometry, "Delete geometry but keep the points").on_hover_text("Removes the input's polygons, leaving its points").changed();
                ui.separator();
                ui.strong("Points");
                let mut remove = None;
                for (i, p) in points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, format!("{i}"), 0.01, &mut p[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut p[k]).speed(0.01)).changed();
                        }
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    points.remove(i);
                    c = true;
                }
                if ui.button("➕ Add point").clicked() {
                    let next = points.last().map_or([0.0; 3], |p| [p[0] + 1.0, p[1], p[2]]);
                    points.push(next);
                    c = true;
                }
                ui.separator();
                ui.strong("Pattern");
                c |= pfield(ui, pslider(count, 0..=100_000).logarithmic(true).text("Point count")).on_hover_text("0: no pattern").changed();
                if *count > 0 {
                    c |= pfield(ui, pslider(spacing, 0.01..=10.0).logarithmic(true).text("Spacing")).changed();
                    ui.horizontal(|ui| {
                        ui.label("Layout");
                        c |= ui.selectable_value(layout, 0, "Line").changed();
                        c |= ui.selectable_value(layout, 1, "Vertical").changed();
                        c |= ui.selectable_value(layout, 2, "Grid").changed();
                    });
                }
            }
            NodeKind::Grid { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Plane { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(height, 0.001..=100.0).logarithmic(true).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=32).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(length, 0.0..=20.0).text("Length")).changed();
                c |= pfield(ui, pslider(rows, 4..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 8..=128).text("Columns")).changed();
            }
            NodeKind::Box { size, div } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(div, 1..=30).text("Divisions")).changed();
            }
            NodeKind::Sphere { radius, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(rows, 2..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Torus { major, minor, rows, cols } => {
                c |= pfield(ui, pslider(major, 0.001..=100.0).logarithmic(true).text("Major Radius")).changed();
                c |= pfield(ui, pslider(minor, 0.0005..=50.0).logarithmic(true).text("Minor Radius")).changed();
                c |= pfield(ui, pslider(rows, 3..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Transform { t, r, s } => {
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= vec3_ui(ui, "Scale    ", s, 0.01);
            }
            NodeKind::Noise { freq, amp } => {
                c |= pfield(ui, pslider(freq, 0.0..=10.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
            }
            NodeKind::Taper { factor } => {
                c |= pfield(ui, pslider(factor, -2.0..=2.0).text("Taper")).changed();
            }
            NodeKind::Twist { angle } => {
                c |= pfield(ui, pslider(angle, -5.0..=5.0).text("Twist / unit")).changed();
            }
            NodeKind::Bend { angle } => {
                c |= pfield(ui, pslider(angle, -3.0..=3.0).text("Bend / unit")).changed();
            }
            NodeKind::Smooth { iterations, strength } => {
                c |= pfield(ui, pslider(iterations, 0..=50).text("Iterations")).changed();
                c |= pfield(ui, pslider(strength, 0.0..=1.0).text("Strength")).changed();
            }
            NodeKind::Bevel { amount, segments } => {
                c |= pfield(ui, pslider(amount, 0.0..=1.0).text("Amount")).changed();
                c |= pfield(ui, pslider(segments, 1..=8).text("Segments")).changed();
            }
            NodeKind::Fuse { tol } => {
                c |= pfield(ui, pslider(tol, 0.0001..=0.5).logarithmic(true).text("Distance"))
                    .changed();
            }
            NodeKind::Mirror { axis } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
            }
            NodeKind::Copy { count, t, r, s } => {
                c |= pfield(ui, pslider(count, 1..=100).text("Copies")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= pfield(ui, pslider(s, 0.5..=1.5).text("Uniform Scale")).changed();
            }
            NodeKind::CopyToPoints { scale, rotation, piece_attribute } => {
                ui.label("Copy the first input onto every point from the second input.");
                c |= pfield(ui, pslider(scale, 0.01..=10.0).logarithmic(true).text("Copy scale")).changed();
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                let mut use_pieces = !piece_attribute.is_empty();
                if ui
                    .checkbox(&mut use_pieces, "Piece attribute")
                    .on_hover_text("Each point copies only the piece of the first input with its value of this attribute")
                    .changed()
                {
                    *piece_attribute = if use_pieces { "variant".into() } else { String::new() };
                    c = true;
                }
                if use_pieces {
                    c |= ui.add(egui::TextEdit::singleline(piece_attribute).desired_width(140.0)).changed();
                    ui.small("Pieces: number them with Enumerate (Primitives, piece attribute name) or Connectivity. Points: Attribute Randomize an integer from 0 to the number of pieces − 1 to choose randomly.");
                }
                ui.small("Use Add or a particle output as the point input.");
            }
            NodeKind::AttributeCreate(a) => {
                ui.label("Creates or replaces an attribute, of any name and type, with one value on every element.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut a.name, &mut a.class, &mut a.kind, &mut a.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (a.name, a.kind, a.size, a.value) = (name.into(), kind, size, value);
                }
                ui.horizontal(|ui| {
                    c |= attribute_value_ui(ui, if a.kind == attrib::QUATERNION { "Rotation°" } else { "Value" }, a.kind, a.size, &mut a.value);
                });
                if a.kind == attrib::QUATERNION {
                    ui.small("Stored as a quaternion (x y z w): turns around X, then Y, then Z, like Transform. Copy to Points uses orient to turn copies.");
                }
            }
            NodeKind::AttributeRandomize(r) if r.preset < randomize::CUSTOM => {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), format!("This node uses an older preset: {}.", randomize::PRESETS[r.preset]));
                if ui.button("Switch to the full settings").on_hover_text("The same values, with any name, type and range").clicked() {
                    *r = r.converted();
                    c = true;
                }
                c |= pfield(ui, pslider(&mut r.amount, 0.0..=10.0).text("Amount")).changed();
                ui.small(match r.preset {
                    2 => "A random colour per point; Amount shifts the hues.",
                    3 => "A random rotation per point (a quaternion), blended in from none by Amount (0 to 1).",
                    _ => "Adds a random value between -Amount and +Amount to each point.",
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeRandomize(r) => {
                ui.label("Random values for an attribute, the same every time for the same seed.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut r.name, &mut r.class, &mut r.kind, &mut r.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (r.name, r.kind, r.size) = (name.into(), kind, size);
                    // A useful range to start from.
                    (r.distribution, r.min, r.max) = match (name, kind) {
                        (_, attrib::QUATERNION) => (randomize::ROTATION, [0.0; 4], [0.0, 360.0, 0.0, 0.0]),
                        ("N" | "v" | "up", _) => (randomize::DIRECTION, [1.0; 4], [1.0; 4]),
                        ("pscale", _) => (0, [value[0] * 0.5; 4], [value[0] * 1.5; 4]),
                        ("id", _) => (0, [0.0; 4], [9.0; 4]),
                        _ => (0, [0.0; 4], [1.0; 4]),
                    };
                }
                let choices = randomize::RandomizeSettings::distributions(r.kind);
                if !choices.contains(&r.distribution) {
                    r.distribution = 0;
                    c = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Values");
                    egui::ComboBox::from_id_salt(("randomize_distribution", id)).selected_text(randomize::DISTRIBUTIONS[r.distribution]).show_ui(ui, |ui| {
                        for i in choices {
                            c |= ui.selectable_value(&mut r.distribution, i, randomize::DISTRIBUTIONS[i]).changed();
                        }
                    });
                });
                match r.distribution {
                    randomize::HUE => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=2.0).text("Hue shift")).changed();
                    }
                    randomize::ROTATION => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=1.0).text("Amount (0 none, 1 any rotation)")).changed();
                    }
                    randomize::DIRECTION => {
                        ui.horizontal(|ui| {
                            c |= pfield(ui, pdrag(&mut r.min[0]).speed(0.01).prefix("Length from ")).changed();
                            c |= pfield(ui, pdrag(&mut r.max[0]).speed(0.01).prefix("to ")).changed();
                        });
                    }
                    _ => {
                        let unit = if r.kind == attrib::QUATERNION { "°" } else { "" };
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Min{unit}"), r.kind, r.size, &mut r.min);
                        });
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Max{unit}"), r.kind, r.size, &mut r.max);
                        });
                        ui.small(match (r.kind, r.distribution) {
                            (attrib::QUATERNION, 1) => "A rotation on the way from Min to Max, in degrees around X, then Y, then Z.",
                            (attrib::QUATERNION, _) => "Each angle between its Min and Max, in degrees around X, then Y, then Z: Max 0 360 0 spins each point around Y.",
                            (_, 1) => "One random point on the way from Min to Max, all components together: a colour between two colours.",
                            (attrib::INTEGER, _) => "Whole numbers from Min to Max, both included.",
                            _ => "Each component on its own, between its Min and Max.",
                        });
                    }
                }
                ui.horizontal(|ui| {
                    for (i, name) in randomize::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut r.mode, i, *name).on_hover_text(match i {
                            0 => "Replace what's there",
                            1 => "Add to what's there (for rotations: turn it further)",
                            _ => "Multiply what's there (for rotations: turn it further)",
                        }).changed();
                    }
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeDelete(d) => {
                ui.label("Removes attributes by name.");
                ui.horizontal(|ui| {
                    ui.label("Names");
                    c |= ui.add(egui::TextEdit::singleline(&mut d.pattern).desired_width(180.0).hint_text("Cd N fx_*")).changed();
                });
                ui.horizontal(|ui| {
                    for (i, name) in attrib::CLASSES.iter().chain(&["All"]).enumerate() {
                        c |= ui.selectable_value(&mut d.class, i, *name).changed();
                    }
                });
                ui.small("Names separated by spaces; * matches anything, and ^ in front keeps a name: \"* ^P ^N\" removes all but those.");
            }
            NodeKind::AttributeVop { domain } => {
                ui.heading("Attribute VOP");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.small("Double-click the node to edit its network. Bind reads attributes, Export writes them.");
                match &vop_code {
                    Some(Ok(code)) => {
                        // Parameter VOPs start at their default value.
                        for (name, default) in &vop_parameters {
                            node.channels.entry(name.clone()).or_insert(*default);
                        }
                        c |= channel_params_ui(ui, code, &mut node.channels);
                        egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                            ui.label(egui::RichText::new(code.as_str()).monospace().small());
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                    }
                    None => {}
                }
            }
            NodeKind::Vop(v) => {
                let def = v.def();
                ui.small(def.help);
                match v.op {
                    vop::BIND | vop::EXPORT | vop::PARAMETER | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        ui.horizontal(|ui| {
                            ui.label(if v.op == vop::PARAMETER { "Parameter" } else { "Attribute" });
                            c |= ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(120.0)).changed();
                            if matches!(v.op, vop::BIND | vop::GET_ATTRIB | vop::PC_AVERAGE) {
                                ui.menu_button("▾", |ui| {
                                    for (name, ty) in [("P", 1), ("N", 1), ("Cd", 1), ("v", 1), ("pscale", 0), ("ptnum", 2), ("primnum", 2), ("numpt", 2), ("Time", 0), ("Frame", 0), ("orient", 3)] {
                                        if ui.button(name).clicked() {
                                            v.name = name.into();
                                            v.ty = ty;
                                            c = true;
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    }
                    vop::SNIPPET => {
                        c |= ui.add(egui::TextEdit::singleline(&mut v.name).code_editor().desired_width(f32::INFINITY).hint_text("$0 * 2 + $1")).changed();
                        ui.small("$0 … $3 are the inputs.");
                    }
                    _ => {}
                }
                match v.op {
                    vop::BIND | vop::CONSTANT | vop::PARAMETER | vop::EXPORT | vop::SNIPPET | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        let choices: &[(&str, char)] = if v.op == vop::PARAMETER { &vop::TYPES[..3] } else { &vop::TYPES };
                        ui.horizontal(|ui| {
                            ui.label("Type");
                            for (i, (name, _)) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *name).changed();
                            }
                        });
                    }
                    vop::COMPONENT => {
                        ui.horizontal(|ui| {
                            ui.label("Component");
                            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *axis).changed();
                            }
                        });
                    }
                    vop::COMPARE => {
                        ui.horizontal(|ui| {
                            ui.label("a");
                            for (i, op) in vop::COMPARES.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *op).changed();
                            }
                            ui.label("b");
                        });
                    }
                    _ => {}
                }
                if matches!(v.op, vop::CONSTANT | vop::PARAMETER) {
                    let label = if v.op == vop::CONSTANT { "Value" } else { "Default" };
                    if v.ty == 1 || v.ty == 3 {
                        c |= vec3_ui(ui, label, &mut v.value, 0.01);
                    } else {
                        c |= pfield(ui, pdrag(&mut v.value[0]).speed(0.01).prefix(format!("{label} "))).changed();
                    }
                }
                if v.reads_geometry() {
                    ui.horizontal(|ui| {
                        ui.label("Geometry");
                        for (i, label) in ["0: being changed", "1", "2", "3"].iter().enumerate() {
                            c |= ui.selectable_value(&mut v.input, i, *label).on_hover_text("Which of the Attribute VOP's inputs it reads").changed();
                        }
                    });
                }
                if !def.inputs.is_empty() {
                    let names: Vec<String> = def.inputs.iter().map(|(n, d)| format!("{n} ({d})")).collect();
                    ui.weak(format!("Inputs, left to right: {}", names.join(", ")));
                }
            }
            NodeKind::Wrangle { code, domain } => {
                ui.heading("VEX Wrangle");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.menu_button("Insert example ▾", |ui| {
                        for (label, example_domain, example) in vex::EXAMPLES {
                            if ui.button(*label).on_hover_text("Replaces the current code").clicked() {
                                *code = example.to_string();
                                *domain = *example_domain;
                                c = true;
                                ui.close_menu();
                            }
                        }
                    });
                    ui.weak("Edits cook live");
                });
                let noun = match *domain {
                    0 => "detail",
                    1 => "primitives",
                    2 => "points",
                    _ => "vertices",
                };
                c |= vex_editor_ui(ui, code, *domain, noun, &mut node.channels, "@P.y += sin(@P.x * 3.0) * 0.2;");
            }
            NodeKind::VolumeWrangle { code } => {
                ui.heading("Volume Wrangle");
                ui.small("Runs over every voxel: @density (fog) or @surface (level set), with @P at the voxel's centre (read-only). volumesample(0, \"density\", pos) reads the volume anywhere; removepoint() switches a voxel off.");
                c |= examples_menu(ui, code, &VOLUME_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "voxels", &mut node.channels, "@density *= 0.5;");
            }
            NodeKind::HeightfieldWrangle { code } => {
                ui.heading("Heightfield Wrangle");
                ui.small("Runs over every point of a terrain: @height and @mask (and the terrain's other attributes), with @P's x and z fixed. volumesample(0, \"height\", pos) and volumeindex() read neighbouring heights.");
                c |= examples_menu(ui, code, &HEIGHTFIELD_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "points", &mut node.channels, "@height += 1;");
            }
            NodeKind::TestGeometry(set) => {
                egui::ComboBox::from_label("Shape").selected_text(testgeo::SHAPES[set.shape.min(testgeo::SHAPES.len() - 1)]).show_ui(ui, |ui| {
                    for (value, label) in testgeo::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape <= 2 {
                    ui.small("Keenan Crane's public-domain (CC0) models, quads with UVs.");
                    c |= ui.checkbox(&mut set.colours, "Painted colours (Cd from the model's texture)").changed();
                }
                if set.shape == testgeo::CHARACTER {
                    ui.small("Quaternius's public-domain (CC0) animated human: a skinned mesh with seven takes.");
                    let clips = testgeo::character_clips();
                    let shown = clips.get(set.clip).cloned().unwrap_or_default();
                    egui::ComboBox::from_label("Take").selected_text(shown).show_ui(ui, |ui| {
                        for (value, label) in clips.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.clip, value, label).changed();
                        }
                    });
                    c |= ui.checkbox(&mut set.looping, "Loop (else hold the last pose)").changed();
                    c |= ui.checkbox(&mut set.colours, "Colours").changed();
                    if set.colours {
                        egui::ComboBox::from_label("Look").selected_text(testgeo::LOOKS[set.look.min(testgeo::LOOKS.len() - 1)]).show_ui(ui, |ui| {
                            for (value, label) in testgeo::LOOKS.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.look, value, *label).changed();
                            }
                        });
                    }
                    ui.small("The take plays from frame 1, at the file's 24 frames per second.");
                }
                c |= pfield(ui, pslider(&mut set.size, 0.01..=100.0).logarithmic(true).text("Size")).on_hover_text("The largest side, in scene units").changed();
                c |= ui.checkbox(&mut set.on_ground, "Sit on the ground (else centred on the origin)").changed();
                if set.shape == 7 {
                    c |= pfield(ui, pslider(&mut set.p, 1..=12).text("Windings (p)")).changed();
                    c |= pfield(ui, pslider(&mut set.q, 1..=12).text("Twists (q)")).changed();
                }
                if set.shape >= 7 {
                    c |= pfield(ui, pslider(&mut set.detail, 8..=256).text("Detail")).changed();
                }
            }
            NodeKind::LSystem(set) => {
                ui.small("Rewrites the premise by the rules, generation after generation, then draws it as curves.");
                ui.menu_button("Presets ▾", |ui| {
                    for (label, premise, rules, generations, angle, step) in lsystem::PRESETS {
                        if ui.button(*label).clicked() {
                            set.premise = premise.to_string();
                            set.rules = rules.to_string();
                            set.generations = *generations;
                            set.angle = *angle;
                            set.step = *step;
                            c = true;
                            ui.close_menu();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Premise");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.premise).code_editor().desired_width(220.0)).changed();
                });
                ui.label("Rules (one per line, e.g. A=F[+A][-A])");
                c |= ui.add(egui::TextEdit::multiline(&mut set.rules).code_editor().desired_rows(4).desired_width(f32::INFINITY)).changed();
                c |= pfield(ui, pslider(&mut set.generations, 0..=12).text("Generations")).changed();
                c |= pfield(ui, pslider(&mut set.angle, 0.0..=180.0).text("Angle")).changed();
                c |= pfield(ui, pslider(&mut set.angle_random, 0.0..=45.0).text("Angle Randomness")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.001..=5.0).logarithmic(true).text("Step Size")).changed();
                c |= pfield(ui, pslider(&mut set.step_scale, 0.0..=1.5).text("Step Scale")).changed();
                c |= pfield(ui, pslider(&mut set.thickness, 0.0..=1.0).text("Thickness")).changed();
                c |= pfield(ui, pslider(&mut set.thickness_scale, 0.0..=1.5).text("Thickness Scale")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                egui::CollapsingHeader::new("Symbols").id_salt("lsystem_symbols").show(ui, |ui| {
                    ui.label(egui::RichText::new(lsystem::SYMBOL_HELP).monospace().small());
                });
            }
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(r_bot, 0.0..=5.0).text("Bottom Radius")).changed();
                c |= pfield(ui, pslider(r_top, 0.0..=5.0).text("Top Radius")).changed();
                c |= pfield(ui, pslider(height, 0.1..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=64).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Peak { dist } => {
                c |= pfield(ui, pslider(dist, -2.0..=2.0).text("Distance")).changed();
            }
            NodeKind::Ripple { freq, amp, speed } => {
                c |= pfield(ui, pslider(freq, 0.0..=20.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
            }
            NodeKind::Jitter { amp, seed, scale } => {
                ui.small("Moves each point by a random offset of up to Amount, scaled per axis (0 keeps that axis still).");
                c |= pfield(ui, pslider(amp, 0.0..=1.0).text("Amount")).changed();
                ui.horizontal(|ui| {
                    ui.label("Axis scale");
                    for (axis, value) in ["x", "y", "z"].iter().zip(scale.iter_mut()) {
                        c |= pfield(ui, pdrag(value).speed(0.01).range(0.0..=10.0).prefix(format!("{axis} "))).changed();
                    }
                });
                c |= pfield(ui, pslider(seed, 0..=999).text("Seed")).changed();
            }
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                ui.small("Input 1: optional source (emits from its surface, or its points; the origin when unconnected) • Input 2: optional static collider.");
                ui.strong("Emission");
                c |= pfield(ui, pslider(count, 1..=50_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(emission_rate, 1.0..=10_000.0).logarithmic(true).text("Birth rate (per second)")).changed();
                c |= pfield(ui, pslider(lifetime, 0.1..=20.0).text("Lifetime")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
                c |= pfield(ui, pslider(spread, 0.0..=2.0).text("Spread")).changed();
                c |= ui
                    .checkbox(along_normals, "Emit along source normals")
                    .on_hover_text("Off: particles head up (+Y). Points inherit their v attribute either way.")
                    .changed();
                ui.strong("Forces");
                c |= pfield(ui, pslider(gravity, -20.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(drag, 0.0..=5.0).text("Drag")).changed();
                ui.strong("Collisions");
                c |= pfield(ui, pslider(bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(friction, 0.0..=1.0).text("Friction")).changed();
                c |= pfield(ui, pslider(radius, 0.0..=0.5).text("Particle radius")).changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                ui.small("Outputs points with v, age, life and id attributes.");
            }
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
                domain,
            } => {
                ui.label("3D CPU FLIP solver");
                c |= pfield(ui, pslider(particle_count, 1..=100_000).logarithmic(true).text("Particles")).changed();
                c |= pfield(ui, pslider(grid_resolution, 16..=128).text("Grid resolution")).changed();
                c |= pfield(ui, pslider(flip_ratio, 0.0..=1.0).text("FLIP blend")).changed();
                c |= pfield(ui, pslider(viscosity, 0.0..=4.0).text("Viscosity")).changed();
                c |= pfield(ui, pslider(gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(max_dt, 0.001..=0.1).logarithmic(true).text("Max timestep")).changed();
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                let mut auto = domain[3] <= 0.0;
                if ui.checkbox(&mut auto, "Domain from the emitter's bounds").on_hover_text("Or drag the domain box's faces in the viewport").changed() {
                    *domain = if auto { [0.0; 6] } else { [0.0, 1.5, 0.0, 4.0, 3.0, 4.0] };
                    c = true;
                }
                if !auto {
                    for (label, range) in [("Domain centre", 0..3), ("Domain size", 3..6)] {
                        ui.horizontal(|ui| {
                            if group_label(ui, label, 0.02, &mut domain[range.clone()]) {
                                c = true;
                                for k in range.clone() {
                                    domain[k] = if k >= 3 { domain[k].clamp(0.01, 1000.0) } else { domain[k].clamp(-1000.0, 1000.0) };
                                }
                            }
                            for k in range {
                                c |= pfield(ui, pdrag(&mut domain[k]).speed(0.02).range(if k >= 3 { 0.01..=1000.0 } else { -1000.0..=1000.0 })).changed();
                            }
                        });
                    }
                }
                ui.small("Input 1: emitter surface • Input 2: optional static collider. Select the node to drag its domain box in the viewport.");
            }
            NodeKind::WhitewaterSolver(w) => {
                ui.small("Spray, foam and bubbles where the fluid input splashes. Input 1: FLIP fluid • Input 2: optional collider. Writes `type` and spray/foam/bubble groups; renders as small spheres.");
                c |= pfield(ui, pslider(&mut w.max_particles, 0..=2_000_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(&mut w.emission_rate, 0.0..=10_000.0).logarithmic(true).text("Emission rate")).changed();
                c |= pfield(ui, pslider(&mut w.trapped_air, 0.0..=4.0).text("Trapped air")).on_hover_text("Emission where fluid collides or tears apart").changed();
                c |= pfield(ui, pslider(&mut w.wave_crest, 0.0..=4.0).text("Wave crests")).on_hover_text("Emission from breaking crests").changed();
                c |= pfield(ui, pslider(&mut w.min_speed, 0.0..=10.0).text("Min speed")).changed();
                c |= pfield(ui, pslider(&mut w.max_speed, 0.1..=30.0).text("Full emission speed")).changed();
                c |= pfield(ui, pslider(&mut w.lifespan, 0.05..=10.0).text("Foam lifespan (s)")).changed();
                c |= pfield(ui, pslider(&mut w.spray_drag, 0.0..=5.0).text("Spray drag")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_buoyancy, 0.0..=10.0).text("Bubble buoyancy")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_drag, 0.0..=1.0).text("Bubble drag")).changed();
                c |= pfield(ui, pslider(&mut w.spray_neighbours, 0..=50).text("Spray below (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_neighbours, 1..=100).text("Bubbles above (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut w.particle_size, 0.0005..=0.2).logarithmic(true).text("Particle size")).changed();
                c |= pfield(ui, pslider(&mut w.substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(&mut w.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                c |= pfield(ui, pslider(radius, 0.001..=1.0).logarithmic(true).text("Particle radius"))
                    .on_hover_text("A lone droplet's radius. 1–1.5× the particles' spacing, which for FLIP is ∛(emitter volume ÷ particles): 100,000 particles in a 1 m cube → about 0.03")
                    .changed();
                c |= pfield(ui, pslider(join_distance, 0.0..=1.0).text("Join distance")).on_hover_text("Extra reach: particles this much further apart still join into one surface. More fills gaps but rounds off splashes").changed();
                c |= pfield(ui, pslider(resolution, 16..=512).text("Resolution")).on_hover_text("Voxels across the particles' longest side. Never finer than a quarter of the radius; very large grids are made coarser").changed();
                c |= pfield(ui, pslider(smoothing, 0..=8).text("Smoothing")).on_hover_text("Passes that even out the surface. 1–2 for water; more melts away thin sheets").changed();
                ui.small("Particles (e.g. FLIP) to one smooth, closed surface. The particles' v, Cd and other point attributes carry onto it. For water: a glass material with IOR 1.33.");
            }
            NodeKind::Extrude(set) => {
                ui.small("Extrudes the node's group (or everything) along the normals. Faces outside the group stay as they are.");
                c |= ui.checkbox(&mut set.connected, "Keep shared points together").on_hover_text("On: connected faces extrude as one, with walls only around the outside. Off: every face on its own").changed();
                c |= pfield(ui, pslider(&mut set.distance, -10.0..=10.0).text("Distance")).changed();
                c |= pfield(ui, pslider(&mut set.inset, 0.0..=1.0).text("Inset")).changed();
                c |= pfield(ui, pslider(&mut set.divisions, 1..=50).text("Divisions")).changed();
                ui.separator();
                ui.strong("Output");
                c |= ui.checkbox(&mut set.output_front, "Output front").changed();
                c |= ui.checkbox(&mut set.output_back, "Output back").on_hover_text("Keep the original faces (flipped), closing the shape").changed();
                c |= ui.checkbox(&mut set.output_side, "Output side").changed();
                c |= ui.checkbox(&mut set.make_groups, "Output groups").changed();
                if set.make_groups {
                    for (label, name) in [("Front group", &mut set.front_group), ("Side group", &mut set.side_group), ("Back group", &mut set.back_group)] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            c |= ui.add(egui::TextEdit::singleline(name).desired_width(140.0)).changed();
                        });
                    }
                }
            }
            NodeKind::Clip { axis, offset, invert } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
                c |= pfield(ui, pslider(offset, -5.0..=5.0).text("Offset")).changed();
                c |= ui.checkbox(invert, "Keep Below").changed();
            }
            NodeKind::Blast { component, indices, invert } => {
                ui.label(format!("Delete {} selected component(s)", indices.len()));
                ui.horizontal(|ui| {
                    ui.label("Component:");
                    c |= ui.selectable_value(component, 0, "Vertices").changed();
                    c |= ui.selectable_value(component, 1, "Faces").changed();
                    c |= ui.selectable_value(component, 2, "Edges").changed();
                });
                c |= ui.checkbox(invert, "Keep selected (delete outside)").changed();
                if ui.button("Clear stored selection").clicked() {
                    indices.clear();
                    c = true;
                }
                ui.small("Select components in the viewport and press Delete to create this node.");
            }
            NodeKind::Cache { cache_id, mode, range } => {
                ui.label("Persistent frame cache: cache/cache_ID_frame.mcache");
                c |= pfield(ui, pslider(cache_id, 0..=9999).text("Cache ID")).changed();
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    c |= ui.selectable_value(mode, 0, "Read").changed();
                    c |= ui.selectable_value(mode, 1, "Write").changed();
                });
                ui.small("Write saves each evaluated frame. Read reuses files and falls back to the input on a miss.");
                ui.label("Frame range (what a Fetch TOP writes when nothing is connected to it)");
                ui.horizontal(|ui| {
                    c |= pfield(ui, pdrag(&mut range[0]).speed(1.0).prefix("Start ")).changed();
                    c |= pfield(ui, pdrag(&mut range[1]).speed(1.0).prefix("End ")).changed();
                    c |= pfield(ui, pdrag(&mut range[2]).speed(0.1).range(0.001..=1000.0).prefix("Step ")).changed();
                });
                ui.small("To write the cache in the background, or across wedges: in TASKS, point a Fetch at this node and cook it. Then set the mode to Read.");
            }
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                ui.heading("VDB from Polygons");
                ui.horizontal(|ui| {
                    ui.label("Output");
                    c |= ui.selectable_value(output, 1, "VDB (level set)").changed();
                    c |= ui.selectable_value(output, 0, "Polygons").changed();
                });
                ui.small(if *output == 1 {
                    "Outputs a signed-distance VDB; use Convert VDB to turn it into polygons or a fog volume."
                } else {
                    "Converts the mesh to a signed-distance volume and outputs its surface directly."
                });
                c |= pfield(ui, pslider(voxel_size, 0.03..=1.0).logarithmic(true).text("Voxel size"))
                    .changed();
                ui.small("Smaller voxels = more detail and slower cooking.");
                c |= pfield(ui, pslider(padding, 0..=4).text("Volume padding")).changed();
                ui.horizontal(|ui| {
                    ui.label("Surface operation");
                    egui::ComboBox::from_id_salt("vdb-operation")
                        .selected_text(match *operation {
                            1 => "Dilate",
                            2 => "Erode",
                            3 => "Smooth",
                            _ => "Rasterize",
                        })
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(operation, 0, "Rasterize").changed();
                            c |= ui.selectable_value(operation, 1, "Dilate").changed();
                            c |= ui.selectable_value(operation, 2, "Erode").changed();
                            c |= ui.selectable_value(operation, 3, "Smooth").changed();
                        });
                });
                if *operation == 1 || *operation == 2 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Surface offset (voxels)")).changed();
                    ui.small(if *operation == 1 {
                        "Moves the surface outward by the selected number of voxels."
                    } else {
                        "Moves the surface inward by the selected number of voxels."
                    });
                } else if *operation == 3 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Smoothing passes")).changed();
                    ui.small("Removes small voxel-scale bumps.");
                } else {
                    ui.small("Creates the base VDB surface without changing its shape.");
                }
                ui.small("The VDB node is evaluated from its first input.");
            }
            NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts } => {
                ui.heading("MoonRay Renderer");
                ui.small("Renders every active geometry with its MAT material, the active lights and the chosen camera, or the displayed USD stage.");
                ui.label("Executable");
                c |= ui.text_edit_singleline(executable).changed();
                ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                c |= ui.text_edit_singleline(arguments).changed();
                ui.label("Output image");
                c |= ui.text_edit_singleline(output).changed();
                if ui.button("Choose output file...").clicked() {
                    if let Some(path) = FileDialog::new().set_file_name("render.exr").save_file() {
                        *output = path.to_string_lossy().into_owned();
                        c = true;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Camera");
                    render_camera_combo(ui, &mut opts.camera, &camera_ids);
                });
                c |= ui.checkbox(&mut opts.stage, "Render the STAGE (USD) instead of the OBJ objects").changed();
                c |= hydra_checkbox(ui, opts);
                c |= pfield(ui, pdrag(width).range(1..=16384).prefix("Width ")).changed();
                c |= pfield(ui, pdrag(height).range(1..=16384).prefix("Height ")).changed();
                c |= pfield(ui, pdrag(samples).range(1..=4096).prefix("Samples per pixel ")).changed();
                c |= pfield(ui, pslider(&mut opts.max_depth, 1..=32).text("Max ray depth")).changed();
                c |= pfield(ui, pslider(&mut opts.light_samples, 1..=16).text("Light samples")).changed();
                c |= pfield(ui, pslider(&mut opts.bsdf_samples, 1..=16).text("BSDF samples")).changed();
                c |= pfield(ui, pslider(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                    .on_hover_text("Light samples per step inside smoke and fire; render time grows roughly in proportion")
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Environment light");
                    c |= ui.color_edit_button_rgb(&mut opts.background).changed();
                });
                ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                ui.separator();
                c |= ui.checkbox(&mut opts.sequence, "Render frame range").changed();
                if opts.sequence {
                    ui.horizontal(|ui| {
                        c |= pfield(ui, pdrag(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start ")).changed();
                        c |= pfield(ui, pdrag(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End ")).changed();
                        c |= pfield(ui, pdrag(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step ")).changed();
                    });
                    if ui.button("Use timeline range").clicked() {
                        opts.range_start = self.playback_start.max(0.0);
                        opts.range_end = self.playback_end.max(opts.range_start);
                        c = true;
                    }
                    let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                    ui.small(format!(
                        "{frames} frames, timeline units. Files: {} (use #### for the frame number position).",
                        frame_output(output, 1)
                    ));
                }
                if let Some(job) = &self.sequence {
                    ui.label(format!("Sequence: frame {}/{}", job.next, job.times.len()));
                    if ui.button("Cancel sequence").clicked() {
                        self.cancel_render();
                    }
                } else if self.moonray_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("MoonRay render in progress...");
                        if ui.button("Cancel").clicked() {
                            self.cancel_render();
                        }
                    });
                } else if ui.button(if opts.sequence { "Render sequence" } else { "Render" }).clicked() {
                    self.start_moonray_render();
                }
                ui.small("Set arguments to match your MoonRay installation. The node passes the exported scene path and output path as placeholders.");
            }
            NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } => {
                ui.label("Viewport camera rig");
                c |= vec3_ui(ui, "Target", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= pfield(ui, pslider(focal, 0.5..=2.5).text("Focal scale")).changed();
                c |= pfield(ui, pslider(near, 0.05..=10.0).text("Near clip")).changed();
                c |= ui.checkbox(orthographic, "Orthographic").changed();
                c |= pfield(ui, pdrag(&mut pan[0]).speed(1.0).prefix("Pan X ")).changed();
                c |= pfield(ui, pdrag(&mut pan[1]).speed(1.0).prefix("Pan Y ")).changed();
                c |= pfield(ui, pslider(zoom, MIN_ZOOM..=200.0).logarithmic(true).text("Zoom")).changed();
                if self.active_camera == Some(id) && self.looking_through {
                    ui.horizontal(|ui| {
                        ui.label("Looking through this camera: moving the view leaves it.");
                        if ui.button("Stop looking through").clicked() {
                            self.look_through_request = Some(None);
                        }
                    });
                } else if self.active_camera == Some(id) {
                    ui.horizontal(|ui| {
                        ui.label("Active viewport camera: moving the viewport moves it.");
                        if ui.button("Stop using as viewport camera").on_hover_text("The viewport keeps its view; this camera stays where it is").clicked() {
                            self.active_camera = None;
                            self.status = "The viewport no longer moves this camera".into();
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("👁 Look through").on_hover_text("See through this camera without changing it; moving the view goes back to the free view").clicked() {
                            self.look_through_request = Some(Some(id));
                        }
                        if ui.button("Use as viewport camera").on_hover_text("Moving the view moves this camera").clicked() {
                            self.use_camera_requested = true;
                        }
                    });
                }
                ui.small("Target is the orbit center used by the procedural viewport.");
            }
            NodeKind::Light { position, rotation, color, intensity, kind } => {
                ui.label("Viewport light");
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= vec3_ui(ui, "Color", color, 0.02);
                c |= pfield(ui, pslider(intensity, 0.0..=4.0).text("Intensity")).changed();
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(kind, 0, "Directional").changed();
                    c |= ui.selectable_value(kind, 1, "Point").changed();
                    c |= ui.selectable_value(kind, 2, "Fill").changed();
                });
                ui.small("Directional lights use rotation. Point lights use position and are evaluated per surface.\nActive lights drive both the viewport and MoonRay; with none active, a default key/fill rig is used.");
            }
            NodeKind::Switch { index } => {
                c |= pfield(ui, pslider(index, 0..=switch_inputs.max(2) - 1).text("Input")).changed();
                ui.small(format!(
                    "{switch_inputs} input(s) connected; 0 passes the first, 1 the second, and so on (past the last, the last). Type an expression to choose by frame, e.g. $F > 24 or $F % {}.",
                    switch_inputs.max(2)
                ));
            }
            NodeKind::Reverse => {
                ui.label("Flips triangle winding (normals).");
            }
            NodeKind::Subdivide { levels } => {
                c |= pfield(ui, pslider(levels, 1..=4).text("Levels")).changed();
            }
            NodeKind::Remesh { target } => {
                c |= pfield(ui, pslider(target, 4..=50_000).logarithmic(true).text("Target triangles"))
                    .changed();
                ui.label("Adds shared-edge subdivisions until the target is reached.");
            }
            NodeKind::HairGuides(set) => {
                ui.small("Input 1: skin mesh. Grows guide curves from its surface along the normals.");
                c |= pfield(ui, pslider(&mut set.count, 1..=20_000).logarithmic(true).text("Guides")).changed();
                c |= pfield(ui, pslider(&mut set.length, 0.0..=5.0).text("Length")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.droop, 0.0..=3.0).text("Droop")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::GuideProcess(set) => {
                ui.small("Shapes the guide curves from input 1. Chain several to combine operations.");
                egui::ComboBox::from_label("Operation").selected_text(groom::GUIDE_OPS[set.op.min(groom::GUIDE_OPS.len() - 1)]).show_ui(ui, |ui| {
                    for (i, label) in groom::GUIDE_OPS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.op, i, *label).changed();
                    }
                });
                let (amount_label, range) = match set.op {
                    0 => ("Length", 0.0..=5.0),
                    1 => ("Bend", 0.0..=5.0),
                    2 => ("Frizz", 0.0..=2.0),
                    3 => ("Curl radius", 0.0..=2.0),
                    _ => ("Smoothing", 0.0..=2.0),
                };
                c |= pfield(ui, pslider(&mut set.amount, range).text(amount_label)).changed();
                if matches!(set.op, 2 | 3) {
                    let label = if set.op == 2 { "Frequency" } else { "Turns per unit length" };
                    c |= pfield(ui, pslider(&mut set.frequency, 0.0..=20.0).text(label)).changed();
                    c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                }
            }
            NodeKind::HairDeform(set) => {
                ui.small("Input 1: hair or guides • Input 2: animated skin. Each strand moves with the skin under its root.");
                c |= pfield(ui, pdrag(&mut set.rest_frame).speed(1.0).range(-10_000.0..=100_000.0).prefix("Rest frame "))
                    .on_hover_text("The frame the hair was groomed on. The skin must keep the same topology at every frame.")
                    .changed();
            }
            NodeKind::HairGenerate(set) => {
                ui.small("Input 1: skin • Input 2: guides. Interpolates dense hair from the three nearest guides.");
                c |= pfield(ui, pslider(&mut set.count, 1..=200_000).logarithmic(true).text("Hairs")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.clump, 0.0..=1.0).text("Clumping")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.frizz, 0.0..=2.0).text("Frizz")).changed();
                c |= pfield(ui, pslider(&mut set.width_root, 0.0..=0.05).text("Root width")).changed();
                c |= pfield(ui, pslider(&mut set.width_tip, 0.0..=0.05).text("Tip width")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                ui.small("Merge the result with the skin to render both.");
            }
            NodeKind::TimeShift { offset, hold, at } => {
                ui.heading("Time Shift");
                ui.small("Input 1 as it is at another time: later or earlier by Offset, or frozen at one moment. Merge shifted copies to build your own loops, or start an effect later.");
                c |= ui.checkbox(hold, "Hold at one time").changed();
                if *hold {
                    c |= pfield(ui, pslider(at, 0.0..=20.0).text("At (s)")).changed();
                    ui.weak(format!("frame {}", (*at * sims::SIM_FPS).round() as i32 + 1));
                } else {
                    c |= pfield(ui, pslider(offset, -10.0..=10.0).text("Offset (s)")).changed();
                    ui.weak(format!("{:+} frames", (*offset * sims::SIM_FPS).round() as i32));
                }
            }
            NodeKind::TimeWarp { speed, anchor } => {
                ui.heading("Time Warp");
                ui.small("Input 1 retimed: time runs Speed times as fast, counting from Anchor, where it's unchanged. 0.5 is half speed. Put a Time Blend after it to smooth a slowed-down simulation.");
                c |= pfield(ui, pslider(speed, -4.0..=4.0).text("Speed")).changed();
                c |= pfield(ui, pslider(anchor, 0.0..=20.0).text("Anchor (s)")).changed();
            }
            NodeKind::TimeBlend => {
                ui.heading("Time Blend");
                ui.small("Simulations step in whole frames, so slowed or shifted by part of a frame they stutter. Between whole frames this mixes the frame before with the one after: particles matched by id, volumes voxel by voxel, other geometry point by point. Put it after a Time Warp or Time Shift.");
            }
            NodeKind::Trail { samples, step, mode, fade } => {
                ui.heading("Trail");
                ui.small("Input 1 over its last few moments, for streaks behind sparks and slashes. Copies merges them, each point with a trail attribute (0 newest, 1 oldest); Curves draws a line through each point's past positions, matched by id; Velocity adds v from how far each point moved.");
                ui.horizontal(|ui| {
                    for (value, label) in [(0, "Copies"), (1, "Curves"), (2, "Velocity")] {
                        c |= ui.selectable_value(mode, value, label).changed();
                    }
                });
                if *mode != 2 {
                    c |= pfield(ui, pslider(samples, 2..=60).text("Samples")).changed();
                }
                c |= pfield(ui, pslider(step, 0.1..=10.0).text("Step (frames)")).changed();
                if *mode == 0 {
                    c |= ui.checkbox(fade, "Fade Alpha and pscale towards the oldest").changed();
                }
            }
            NodeKind::LoopParticles { start, length } => {
                ui.heading("Loop Particles");
                ui.small("Makes a particle simulation (input 1) loop exactly, for looping game effects: no blending, so nothing ghosts. Set the playback range to Start until Start + Length and export it as a sprite sheet. The particles mustn't affect each other, and by Start the emitter should have been running for at least a particle's lifetime.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                ui.weak(format!("{} frames", (*length * sims::SIM_FPS).round()));
            }
            NodeKind::MakeLoop { start, length, blend } => {
                ui.heading("Make Loop");
                ui.small("Makes a simulation (input 1) loop, such as Pyro: it carries on past the end and blends that into the start, voxel by voxel or point by point, over Blend seconds. Set the playback range to Start until Start + Length. For particles, Loop Particles loops exactly.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                c |= pfield(ui, pslider(blend, 0.0..=*length).text("Blend (s)")).changed();
                ui.weak(format!("{} frames, blending over the first {}", (*length * sims::SIM_FPS).round(), (*blend * sims::SIM_FPS).round()));
                if ui.button("Find best length").on_hover_text("Tries lengths from half to twice this one and picks the one where the effect comes back closest to how it started, so the blend has least to hide. Keeps this length if none comes back. Simulates up to twice the length past Start.").clicked() {
                    self.loop_search_request = Some(id);
                }
            }
            NodeKind::Pyro(set) => {
                ui.heading("Pyro Solver");
                ui.small("Smoke and fire on a voxel grid. Input 1: source (closed mesh or points, pscale sets point radius) • Input 2: optional collider (closed mesh).");
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    for (label, preset) in set.presets() {
                        if ui.button(label).clicked() {
                            *set = preset;
                            c = true;
                        }
                    }
                });
                ui.separator();
                ui.strong("Domain");
                c |= vec3_ui(ui, "Center", &mut set.center, 0.05);
                c |= vec3_ui(ui, "Size", &mut set.size, 0.05);
                if ui.button("Fit around source").on_hover_text("Centre the box on the source and leave room for the smoke to rise").clicked() {
                    self.pyro_fit_request = Some(id);
                }
                c |= pfield(ui, pslider(&mut set.resolution, 8..=160).text("Resolution (voxels on longest side)")).changed();
                let voxels = {
                    let h = set.size.iter().cloned().fold(0.0, f32::max) / set.resolution.max(4) as f32;
                    set.size.iter().map(|s| ((s / h).ceil() as usize).max(4)).product::<usize>()
                };
                ui.weak(format!("{voxels} voxels"));
                c |= pfield(ui, pslider(&mut set.substeps, 1..=8).text("Substeps")).changed();
                ui.separator();
                ui.strong("Source");
                c |= pfield(ui, pslider(&mut set.density, 0.0..=4.0).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.temperature, 0.0..=4.0).text("Temperature")).changed();
                c |= pfield(ui, pslider(&mut set.fuel, 0.0..=4.0).text("Fuel (fire)")).changed();
                ui.strong("Shape");
                c |= pfield(ui, pslider(&mut set.buoyancy, -2.0..=5.0).text("Buoyancy")).changed();
                c |= pfield(ui, pslider(&mut set.dissipation, 0.0..=3.0).text("Dissipation")).changed();
                c |= pfield(ui, pslider(&mut set.cooling, 0.0..=5.0).text("Cooling")).changed();
                c |= pfield(ui, pslider(&mut set.burn_rate, 0.0..=5.0).text("Burn rate")).changed();
                c |= pfield(ui, pslider(&mut set.vorticity, 0.0..=3.0).text("Vorticity")).changed();
                c |= pfield(ui, pslider(&mut set.turbulence, 0.0..=3.0).text("Turbulence")).changed();
                c |= vec3_ui(ui, "Wind", &mut set.wind, 0.05);
                c |= pfield(ui, pslider(&mut set.opacity, 0.0..=4.0).text("Display opacity")).changed();
                ui.small("Output: points with density, temperature, flame, Cd, Alpha and pscale, drawn as soft volume splats. Edits restart the simulation.");
            }
            NodeKind::GroupCreate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Group name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Points, "Points").changed();
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Prims, "Primitives").changed();
                });
                egui::ComboBox::from_label("Method").selected_text(groups::CREATE_METHODS[set.method.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in groups::CREATE_METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *label).changed();
                    }
                });
                match set.method {
                    0 => {
                        c |= vec3_ui(ui, "Min", &mut set.bbox_min, 0.05);
                        c |= vec3_ui(ui, "Max", &mut set.bbox_max, 0.05);
                    }
                    1 => {
                        c |= pfield(ui, pslider(&mut set.percent, 0.0..=100.0).text("Percent")).changed();
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                    }
                    2 => {
                        c |= vec3_ui(ui, "Direction", &mut set.direction, 0.05);
                        c |= pfield(ui, pslider(&mut set.spread, 0.0..=180.0).text("Spread angle")).changed();
                    }
                    _ => {
                        c |= ui.add(egui::TextEdit::singleline(&mut set.expression).hint_text("@P.y>0  0-10  other ^skip").desired_width(220.0)).changed();
                    }
                }
                ui.horizontal(|ui| {
                    group_pick_ui(ui, self.group_pick.as_ref(), (self.selected_vertices.len(), self.selected_triangles.len()), id, PickTarget::GroupCreate, false, &mut self.group_pick_request, &mut self.group_pick_domain, &mut self.group_pick_finish);
                    ui.small("Picking sets the method to Expression, with the picked numbers.");
                });
                egui::ComboBox::from_label("Combine").selected_text(groups::COMBINE_MODES[set.combine.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in groups::COMBINE_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                ui.small("Stored as a 0/1 attribute (group:name / prim:group:name); use the name in any node's Group field.");
            }
            NodeKind::AttributePromote(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd"))
                        .on_hover_text("Attributes of the original class, separated by spaces: * for all, ^name to skip one. P (named) gives each polygon its centre")
                        .changed();
                });
                for (label, class) in [("From", &mut set.from), ("To", &mut set.to)] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        for (value, name) in promote::CLASSES.iter().enumerate() {
                            c |= ui.selectable_value(class, value, *name).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Promotion method").selected_text(promote::METHODS[set.method.min(promote::METHODS.len() - 1)]).show_ui(ui, |ui| {
                    for (value, name) in promote::METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("New name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.rename).desired_width(140.0).hint_text("same name")).changed();
                });
                c |= ui.checkbox(&mut set.delete_original, "Delete original").changed();
                ui.small("Where several values meet (a point shared by polygons, a polygon's points, everything for the detail), the method combines them. A quad is one primitive with four vertices.");
            }
            NodeKind::AttributeBlur(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd mask"))
                        .on_hover_text("Point attributes, separated by spaces: * for all, ^name to skip one. P (named) smooths the shape itself")
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Neighbours");
                    for (value, label) in blur::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Weighting");
                    for (value, label) in blur::WEIGHTINGS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).on_hover_text("Edge length: nearer neighbours count more").changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.iterations, 0..=200).logarithmic(true).text("Blurring iterations")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.0..=1.0).text("Step size")).on_hover_text("How far each pass moves toward the neighbours' average").changed();
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.radius, 0.001..=10.0).logarithmic(true).text("Radius")).changed();
                    c |= pfield(ui, pslider(&mut set.max_points, 1..=100).text("Max points")).changed();
                } else {
                    c |= ui.checkbox(&mut set.pin_border, "Pin border").on_hover_text("Points on open edges keep their values").changed();
                }
                ui.small("Connectivity: points sharing a polygon edge. Proximity: the nearest points within the radius (works on loose points and particles). The group's points change; the others feed in but stay as they are.");
            }
            NodeKind::Enumerate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in enumerate::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Piece attribute");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.piece_attribute).desired_width(140.0).hint_text("optional"))
                        .on_hover_text("Elements sharing a value of this attribute share a number: the pieces are numbered, in the order they first appear")
                        .changed();
                });
                c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start ")).changed();
                c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment ")).changed();
                ui.small("Writes 0, 1, 2… (from Start, by Increment) on each point or primitive (a polygon counts once). For Copy to Points' piece attribute: Class Primitives on the shapes to copy.");
            }
            NodeKind::Color(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in color::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Color type").selected_text(color::MODES[set.mode.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in color::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    1 => {
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                        ui.horizontal(|ui| {
                            ui.label("One colour per");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.random_by).hint_text("element, or e.g. piece").desired_width(130.0)).changed();
                        });
                    }
                    2 => {
                        ui.horizontal(|ui| {
                            ui.label("From");
                            c |= ui.color_edit_button_rgb(&mut set.color).changed();
                            ui.label("to");
                            c |= ui.color_edit_button_rgb(&mut set.color2).changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label("Along");
                            for (value, label) in color::AXES.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                            }
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label("Color");
                            c |= ui.color_edit_button_rgb(&mut set.color).changed();
                        });
                    }
                }
                ui.small("Sets Cd (prim:Cd for primitives). Use the Group field above to colour only part of the geometry.");
            }
            NodeKind::AttributeTransfer(set) => {
                ui.small("Copies attributes from the second input onto the nearest points or primitives of the first.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in transfer::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.pattern).hint_text("* or Cd temperature ^id").desired_width(160.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.distance, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Distance threshold"))
                    .on_hover_text("Elements farther than this from every source keep what they had").changed();
                c |= pfield(ui, pslider(&mut set.blend_width, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Blend width"))
                    .on_hover_text("Past the threshold, the transfer fades out over this distance instead of stopping at a hard edge").changed();
                c |= pfield(ui, pslider(&mut set.samples, 1..=32).text("Max samples"))
                    .on_hover_text("How many nearby sources are blended, weighted by distance (1: the nearest only)").changed();
            }
            NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } => {
                ui.heading("Environment Light");
                ui.small("Lights the scene from a lat-long HDRI (.exr or .hdr), in MoonRay and the viewport.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/sky.exr").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("HDRI", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if !file.is_empty() {
                    if let Err(e) = envlight::load(file) {
                        ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= pfield(ui, pslider(intensity, 0.0..=10.0).text("Intensity")).changed();
                c |= pfield(ui, pslider(rotation, -180.0..=180.0).text("Rotate (Y°)")).changed();
                c |= ui.checkbox(visible, "Visible in camera (background)").changed();
            }
            NodeKind::AttributePaint { attribute, values, settings: set } => {
                let painting = self.paint_active;
                if ui
                    .selectable_label(painting, if painting { "🖌 Painting — drag in the viewport (Ctrl erases)" } else { "🖌 Paint in viewport" })
                    .on_hover_text("While on, dragging in the viewport paints instead of orbiting. Right/middle drag still pans.")
                    .clicked()
                {
                    self.paint_active = !painting;
                }
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(attribute).desired_width(110.0)).changed();
                    egui::ComboBox::from_id_salt("paint_presets").selected_text("Groom ▾").show_ui(ui, |ui| {
                        for name in paint::GROOM_ATTRIBUTES {
                            if ui.selectable_label(attribute == name, name).clicked() {
                                *attribute = name.to_owned();
                                c = true;
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for (value, label) in paint::PAINT_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.value, -2.0..=4.0).text("Paint value")).changed();
                c |= pfield(ui, pslider(&mut set.default, -2.0..=4.0).text("Default (unpainted)")).changed();
                c |= pfield(ui, pslider(&mut set.radius, 2.0..=400.0).logarithmic(true).text("Brush radius (px)")).changed();
                c |= pfield(ui, pslider(&mut set.strength, 0.01..=1.0).text("Strength")).changed();
                c |= ui.checkbox(&mut set.visualize, "Show as colour (Cd)").changed();
                ui.horizontal(|ui| {
                    if ui.button("Fill with paint value").clicked() {
                        // Needs the input's point count; done at the start of the next frame.
                        self.paint_fill = Some(id);
                    }
                    if ui.button("Clear").clicked() {
                        values.clear();
                        c = true;
                    }
                });
                ui.small(format!(
                    "{} painted point(s). The groom reads density, length, clump, frizz and mask from the skin.",
                    values.len()
                ));
            }
            NodeKind::Normal(set) => {
                ui.horizontal(|ui| {
                    ui.label("Add normals to");
                    for (value, label) in sops::NORMAL_DOMAINS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.domain, value, *label).changed();
                    }
                });
                if set.domain == 1 {
                    c |= pfield(ui, pslider(&mut set.cusp_angle, 0.0..=180.0).text("Cusp angle"))
                        .on_hover_text("Faces meeting at a sharper angle than this keep a hard edge")
                        .changed();
                }
                egui::ComboBox::from_label("Weighting").selected_text(sops::NORMAL_WEIGHTS[set.weighting.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in sops::NORMAL_WEIGHTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.reverse, "Reverse normals").changed();
                ui.small("The viewport shades with these normals (vertex N, then point N).");
            }
            NodeKind::Scatter(set) => {
                ui.small("Scatters points on the input's surface, or inside it when it's a closed mesh. Points get N, pscale and sourceprim.");
                c |= pfield(ui, pslider(&mut set.count, 1..=1_000_000).logarithmic(true).text("Points")).changed();
                c |= ui.checkbox(&mut set.volume, "Fill the inside (closed meshes)").changed();
                c |= pfield(ui, pslider(&mut set.relax_iterations, 0..=50).text("Relax iterations")).on_hover_text("Spreads the points out evenly").changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Boolean(set) => {
                ui.small("Combines input A (left) with input B (right). Both should be closed meshes. Result faces are in the a_faces and b_faces primitive groups.");
                egui::ComboBox::from_label("Operation").selected_text(boolean::OPERATIONS[set.operation.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::OPERATIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.operation, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Method").selected_text(boolean::MODES[set.mode.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.resolution, 16..=400).text("Voxel resolution")).on_hover_text("Voxels along the longest side").changed();
                } else {
                    ui.small("Exact keeps sharp edges. Use Volume for very dense or leaky meshes.");
                }
            }
            NodeKind::PointDeform(set) => {
                ui.small("Input 1: geometry • Input 2: rest points • Input 3: the same points, moved (a simulation, a lattice, an animated cloud). The geometry follows them, bending and turning.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=32).text("Points followed")).changed();
                c |= pfield(ui, pslider(&mut set.fit_neighbours, 3..=64).text("Rotation from neighbours")).changed();
                c |= ui.checkbox(&mut set.rotate, "Follow rotation").changed();
            }
            NodeKind::TransformPieces => {
                ui.small("Input 1: geometry with `piece` (or `name`) • Input 2: one point per piece with P, orient or transform, pscale • Input 3 (optional): the template points at rest. Each piece moves rigidly with its point.");
            }
            NodeKind::ExtractCentroid(set) => {
                ui.small("One point per piece, at its centre (with a piece attribute): feed it to Copy to Points or Transform Pieces.");
                egui::ComboBox::from_label("Pieces").selected_text(sopkit::PARTITIONS[set.partition.min(2)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::PARTITIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.partition, i, *name).changed();
                    }
                });
                egui::ComboBox::from_label("Centre").selected_text(sopkit::CENTRES[set.method.min(1)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::CENTRES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, i, *name).changed();
                    }
                });
            }
            NodeKind::MatchSize(set) => {
                ui.small(if has_reference { "Lines the input up with input 2's bounding box." } else { "Lines the input up with the target box below (or connect a reference to input 2)." });
                for (a, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Justify {axis}"));
                        for (j, name) in sopkit::JUSTIFY.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.justify[a], j, *name).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.scale_to_fit, "Scale to fit").changed();
                if set.scale_to_fit {
                    c |= ui.checkbox(&mut set.uniform, "Uniform scale (keep proportions)").changed();
                }
                if !has_reference {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, "Target centre", 0.01, &mut set.target_center[..]);
                        for v in set.target_center.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01)).changed();
                        }
                    });
                    ui.horizontal(|ui| {
                        if group_label(ui, "Target size", 0.01, &mut set.target_size[..]) {
                            c = true;
                            set.target_size.iter_mut().for_each(|v| *v = v.max(0.0));
                        }
                        for v in set.target_size.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01).range(0.0..=1e6)).changed();
                        }
                    });
                }
            }
            NodeKind::ConvertLine => {
                ui.small("Every polygon edge (not the diagonals inside quads) as a two-point line, with a restlength attribute. Lines are curves: Resample, Copy to Points and grooming work on them.");
            }
            NodeKind::Metaball(set) => {
                ui.small("Soft blobs around the input's points (pscale scales each), joined where they meet; with no input, one ball at the origin.");
                c |= pfield(ui, pslider(&mut set.radius, 0.01..=10.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(&mut set.threshold, 0.01..=0.99).text("Threshold")).on_hover_text("Lower is fatter: blobs reach out and blend sooner").changed();
                c |= pfield(ui, pslider(&mut set.resolution, 8..=256).text("Resolution")).changed();
            }
            NodeKind::Connectivity(set) => {
                ui.small("Numbers the connected pieces 0, 1, 2… in an attribute.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    c |= ui.selectable_value(&mut set.class, 0, "Points").changed();
                    c |= ui.selectable_value(&mut set.class, 1, "Primitives").changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(120.0)).changed();
                });
            }
            NodeKind::Resample(set) => {
                ui.small("Evenly spaced points along curves (Convert Line, grooms…); attributes are interpolated.");
                c |= ui.checkbox(&mut set.by_length, "Maximum segment length").changed();
                if set.by_length {
                    c |= pfield(ui, pslider(&mut set.length, 0.001..=10.0).logarithmic(true).text("Length")).changed();
                } else {
                    c |= pfield(ui, pslider(&mut set.segments, 1..=500).text("Segments")).changed();
                }
            }
            NodeKind::Measure(set) => {
                egui::ComboBox::from_label("Measure").selected_text(measure::KINDS[set.kind.min(6)]).show_ui(ui, |ui| {
                    for (value, label) in measure::KINDS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                ui.small(match set.kind {
                    0 => "Each primitive's area (prim attribute).",
                    1 => "Each primitive's perimeter; the Sum summary is the mesh's open boundary length.",
                    2 => "Each point's share of the surface area.",
                    3 => "Per point: mean curvature (1/radius on a sphere, + convex) or Gaussian (1/radius², 0 on flat or cylinders).",
                    4 => "Per point: the gradient (direction and rate of fastest increase) of a float attribute, as a vector.",
                    5 => "The volume of the closed mesh (detail attribute).",
                    _ => "Per point: the length of the curve it's on.",
                });
                if set.kind == 3 {
                    ui.horizontal(|ui| {
                        for (value, label) in measure::CURVATURES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.curvature, value, *label).changed();
                        }
                    });
                }
                if set.kind == 4 {
                    ui.horizontal(|ui| {
                        ui.label("Of attribute");
                        c |= ui.add(egui::TextEdit::singleline(&mut set.source).desired_width(120.0)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Output name");
                    let hint = set.output_name();
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).hint_text(hint).desired_width(120.0)).changed();
                });
                if set.kind != 5 {
                    egui::ComboBox::from_label("Detail summary").selected_text(measure::SUMMARIES[set.summary.min(4)]).show_ui(ui, |ui| {
                        for (value, label) in measure::SUMMARIES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.summary, value, *label).changed();
                        }
                    });
                }
            }
            NodeKind::ForEachBegin | NodeKind::CompileBegin => {
                ui.small(if matches!(node.kind, NodeKind::ForEachBegin) {
                    "Puts out each iteration's geometry (detail attributes iteration, numiterations and value). Set the loop up on its For-Each End."
                } else {
                    "Start of a Compile Block: For-Each loops inside it run on several threads, and the block's result is cached."
                });
                if let Some(end) = &loop_begin_name {
                    ui.label(format!("Ends at {end}"));
                }
            }
            NodeKind::ForEachEnd(set) => {
                ui.small("Runs the nodes between its For-Each Begin and here once per iteration, merging the results (or feeding each result to the next, for feedback).");
                egui::ComboBox::from_label("Loop over").selected_text(flow::MODES[set.mode.min(5)]).show_ui(ui, |ui| {
                    for (value, label) in flow::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    flow::NAMED => {
                        ui.horizontal(|ui| {
                            ui.label("Piece attribute");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).hint_text("name").desired_width(120.0)).changed();
                        });
                        ui.small("A primitive or point attribute, e.g. piece (Material Fracture) or name.");
                    }
                    flow::NUMBER | flow::FEEDBACK => {
                        c |= pfield(ui, pdrag(&mut set.iterations).range(0..=100_000).prefix("Iterations: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start value: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment: ")).changed();
                    }
                    _ => {}
                }
                let mut single = set.single_pass >= 0;
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut single, "Single pass").on_hover_text("Run only one iteration, to check it").changed();
                    if single {
                        let mut pass = set.single_pass.max(0);
                        c |= pfield(ui, pdrag(&mut pass).range(0..=1_000_000)).changed();
                        set.single_pass = pass;
                    } else {
                        set.single_pass = -1;
                    }
                });
                if set.mode != flow::FEEDBACK {
                    c |= ui.checkbox(&mut set.multithread, "Multithread when compiled").changed();
                    ui.small(if compiled_loop { "This loop is inside a Compile Block: iterations run in parallel." } else { "Put the loop inside a Compile Block to run its iterations in parallel." });
                }
                match &loop_begin_name {
                    Some(begin) => ui.label(format!("Begins at {begin}")),
                    None => ui.colored_label(egui::Color32::from_rgb(240, 120, 100), "No For-Each Begin: add the loop again from Flow Control."),
                };
            }
            NodeKind::CompileEnd { .. } => {
                ui.small("End of a Compile Block: the block cooks as a unit, its result is cached, and For-Each loops inside it run on several threads.");
                if let Some(begin) = &loop_begin_name {
                    ui.label(format!("Begins at {begin}"));
                }
            }
            NodeKind::ObjectMerge { sources, into_this } => {
                ui.small("Brings in an object's geometry (its display output), or any node's, in another object or this one.");
                let mut remove = None;
                for (i, source) in sources.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // "object" for an object's display output, "object / node" for a node inside it.
                        let shown = source
                            .and_then(|s| {
                                merge_sources.iter().find_map(|(object, name, nodes)| {
                                    if *object == s {
                                        Some(name.clone())
                                    } else {
                                        nodes.iter().find(|(n, _)| *n == s).map(|(_, path)| format!("{name} / {path}"))
                                    }
                                })
                            })
                            .unwrap_or_else(|| "(choose)".to_owned());
                        ui.push_id(("object_merge", id, i), |ui| {
                            ui.menu_button(format!("{shown} ⏷"), |ui| {
                                if merge_sources.is_empty() {
                                    ui.weak("No geometry objects");
                                }
                                for (object, name, nodes) in &merge_sources {
                                    ui.menu_button(name, |ui| {
                                        let own = Some(*object) == merge_own_object;
                                        let display = ui
                                            .add_enabled(!own, egui::Button::selectable(*source == Some(*object), "Display output"))
                                            .on_hover_text("The object's display node, with its transform")
                                            .on_disabled_hover_text("An Object Merge can't merge its own object; pick a node in it instead");
                                        if display.clicked() {
                                            *source = Some(*object);
                                            c = true;
                                            ui.close();
                                        }
                                        if !nodes.is_empty() {
                                            ui.separator();
                                        }
                                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                            for (n, path) in nodes {
                                                if ui.add(egui::Button::selectable(*source == Some(*n), path)).clicked() {
                                                    *source = Some(*n);
                                                    c = true;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                        });
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    sources.remove(i);
                    c = true;
                }
                if ui.button("➕ Add object").clicked() {
                    sources.push(None);
                    c = true;
                }
                c |= ui.checkbox(into_this, "Into this object").on_hover_text("Keep each object's position in the scene (off: its geometry as built, without its transform)").changed();
            }
            NodeKind::MaterialAssign { slots } => {
                ui.small("Assigns MAT materials to parts of the geometry: each slot's group (empty: everything) gets its material, overriding the object's. Later slots win.");
                let mut remove = None;
                for (i, (group, material)) in slots.iter_mut().enumerate() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("Group {}", i + 1));
                        c |= ui.add(egui::TextEdit::singleline(group).hint_text("all").desired_width(120.0)).changed();
                        if ui.small_button("✕").on_hover_text("Remove this slot").clicked() {
                            remove = Some(i);
                        }
                    });
                    group_pick_ui(ui, self.group_pick.as_ref(), (self.selected_vertices.len(), self.selected_triangles.len()), id, PickTarget::MaterialSlot(i), true, &mut self.group_pick_request, &mut self.group_pick_domain, &mut self.group_pick_finish);
                    let shown = material.and_then(|m| material_ids.iter().find(|(mid, _)| *mid == m)).map_or("(object's material)".to_owned(), |(_, name)| name.clone());
                    ui.horizontal(|ui| {
                        ui.label("Material");
                        egui::ComboBox::from_id_salt(("material_slot", id, i)).selected_text(shown).show_ui(ui, |ui| {
                            c |= ui.selectable_value(material, None, "(object's material)").changed();
                            for (mid, name) in &material_ids {
                                c |= ui.selectable_value(material, Some(*mid), name).changed();
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    slots.remove(i);
                    c = true;
                }
                if ui.button("➕ Add slot").clicked() {
                    slots.push((String::new(), None));
                    c = true;
                }
                ui.small("Groups: names (from Group Create or Boolean's a_faces/b_faces), ranges like 0-99, or @Cd.r>0.5. Make materials in the MAT context.");
            }
            NodeKind::MaterialFracture(set) => {
                ui.small("Breaks a closed mesh into pieces for the RBD Solver. Pieces get `piece`; cut faces are in the `inside` group, the original surface in `outside`.");
                egui::ComboBox::from_label("Material").selected_text(fracture::MATERIALS[set.material.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in fracture::MATERIALS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.material, value, *label).changed();
                    }
                });
                match set.material {
                    1 => {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, "Impact point", 0.01, &mut set.impact[..]);
                            for k in 0..3 {
                                c |= pfield(ui, pdrag(&mut set.impact[k]).speed(0.01)).changed();
                            }
                        });
                        c |= pfield(ui, pslider(&mut set.rings, 1..=20).text("Crack rings")).changed();
                        c |= pfield(ui, pslider(&mut set.spokes, 3..=64).text("Radial cracks")).changed();
                    }
                    m => {
                        c |= pfield(ui, pslider(&mut set.pieces, 2..=500).logarithmic(true).text("Pieces")).changed();
                        if m == 2 {
                            ui.horizontal(|ui| {
                                ui.label("Grain along");
                                for (value, label) in sops::AXES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut set.grain_axis, value, *label).changed();
                                }
                            });
                            c |= pfield(ui, pslider(&mut set.splinter, 1.0..=20.0).text("Splinter length")).changed();
                        }
                    }
                }
                c |= pfield(ui, pslider(&mut set.separate, 0.0..=1.0).text("Explode view")).on_hover_text("Moves pieces apart to see the cracks").changed();
                c |= pfield(ui, pslider(&mut set.glue_strength, 0.0..=100_000.0).logarithmic(true).text("Glue strength")).on_hover_text("How hard the constraints between pieces hold in the RBD Bullet Solver").changed();
                ui.small("Outputs: 1 pieces · 2 glue constraints (lines between neighbouring pieces) · 3 proxy collision shapes. Wire all three into an RBD Bullet Solver.");
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::RbdBullet(set) => {
                ui.small("Inputs: 1 pieces (piece attribute) · 2 glue constraints · 3 proxy collision shapes · 4 static collision geometry. Wire Material Fracture's three outputs straight in. Outputs the moved pieces, the constraints still holding, and the moved proxies.");
                c |= pfield(ui, pslider(&mut set.density, 0.01..=100.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.glue_scale, 0.0..=100.0).logarithmic(true).text("Glue strength ×")).on_hover_text("Scales every constraint's strength; 0 breaks all glue at the start").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=32).text("Substeps")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                if set.ground {
                    c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.01).prefix("Ground height: ")).changed();
                }
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Initial velocity", 0.05, &mut set.velocity[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                    }
                });
                ui.small("RBD Configure sets density, friction, bounce, active and velocity per piece.");
            }
            NodeKind::RbdConfigure(set) => {
                ui.small("Sets physical properties on the pieces in the node's group (all when empty), for the RBD Bullet Solver. Constraints and proxy pass through.");
                for (on, label, value, range) in [
                    (&mut set.set_density, "Density", &mut set.density, 0.01..=100.0),
                    (&mut set.set_friction, "Friction", &mut set.friction, 0.0..=2.0),
                    (&mut set.set_bounce, "Bounce", &mut set.bounce, 0.0..=1.0),
                ] {
                    ui.horizontal(|ui| {
                        c |= ui.checkbox(on, "").changed();
                        c |= ui.add_enabled(*on, pslider(value, range).text(label)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_active, "").changed();
                    c |= ui.add_enabled(set.set_active, egui::Checkbox::new(&mut set.active, "Active (off: static, others still hit it)")).changed();
                });
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_velocity, "").changed();
                    ui.add_enabled_ui(set.set_velocity, |ui| {
                        c |= group_label(ui, "Velocity", 0.05, &mut set.velocity[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                        }
                    });
                });
            }
            NodeKind::Pack(set) => {
                ui.small("Turns each piece into one point (P, transform, piece) whose geometry is stored once: cheap to move, copy and merge. The viewport and renders show the real geometry; Unpack turns it back.");
                egui::ComboBox::from_label("Pack").selected_text(packed::MODES[set.mode.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in packed::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pivot");
                    for (value, label) in packed::PIVOTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.pivot, value, *label).changed();
                    }
                });
            }
            NodeKind::Unpack { transfer } => {
                ui.small("Turns packed points back into geometry at their transform (P, transform, or orient and pscale).");
                c |= ui.checkbox(transfer, "Transfer the packed points' attributes (Cd, piece…)").changed();
            }
            NodeKind::CrowdSolver(set) => {
                ui.small("Input 1: start points (e.g. Scatter on a grid) • Input 2: optional ground to walk on • Input 3: optional obstacles.");
                ui.horizontal(|ui| {
                    ui.label("Display");
                    for (value, label) in crowd::DISPLAYS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.display, value, *label).changed();
                    }
                });
                if set.display == 0 {
                    ui.small("For your own characters: feed these points to Copy to Points's second input, with a character facing +Z, feet at the origin, into its first. orient turns each one to where its agent walks, and pscale sizes it to the agent height (for a character 1.75 tall).");
                }
                c |= pfield(ui, pslider(&mut set.speed, 0.0..=10.0).text("Walk speed")).changed();
                c |= pfield(ui, pslider(&mut set.speed_variance, 0.0..=1.0).text("Speed variance")).changed();
                c |= pfield(ui, pslider(&mut set.turn_rate, 1.0..=720.0).text("Turn rate°/s")).changed();
                c |= pfield(ui, pslider(&mut set.personal_space, 0.05..=5.0).text("Personal space")).changed();
                c |= pfield(ui, pslider(&mut set.separation, 0.0..=10.0).text("Separation")).changed();
                c |= pfield(ui, pslider(&mut set.alignment, 0.0..=5.0).text("Alignment")).changed();
                c |= pfield(ui, pslider(&mut set.cohesion, 0.0..=5.0).text("Cohesion")).changed();
                c |= pfield(ui, pslider(&mut set.wander, 0.0..=5.0).text("Wander")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Goal", 0.1, &mut set.goal[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.goal[k]).speed(0.1)).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.goal_weight, 0.0..=10.0).text("Goal strength")).changed();
                c |= pfield(ui, pslider(&mut set.avoid_distance, 0.0..=10.0).text("Obstacle look-ahead")).changed();
                c |= pfield(ui, pslider(&mut set.agent_height, 0.1..=5.0).text("Agent height")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Skeleton(set) => {
                ui.small("A skeleton: joints as points with `parent` and `transform`. Pose it with Rig Pose, Two Bone IK or Procedural Motion; skin a mesh with Joint Capture and Bone Deform.");
                egui::ComboBox::from_label("Preset").selected_text(kinefx::PRESETS[set.preset.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::PRESETS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.preset, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.scale, 0.01..=10.0).logarithmic(true).text("Scale")).changed();
                if set.preset != 0 {
                    c |= pfield(ui, pslider(&mut set.joints, 2..=200).text("Joints")).changed();
                    c |= pfield(ui, pslider(&mut set.length, 0.01..=50.0).logarithmic(true).text("Length")).changed();
                }
            }
            NodeKind::RigPose(pose) => {
                ui.small("Rotates (degrees, in the joint's own axes) and moves joints; children follow. Every value can be keyframed.");
                for slot in 0..kinefx::POSE_SLOTS {
                    ui.separator();
                    c |= joint_menu(ui, ("pose", id, slot), &format!("Joint {}", slot + 1), &mut pose.joints[slot], &joint_names, true);
                    if pose.joints[slot].trim().is_empty() {
                        continue;
                    }
                    let v = &mut pose.values[slot];
                    for (label, range) in [("Rotate", 0..3), ("Translate", 3..6)] {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, label, if range.start < 3 { 0.5 } else { 0.01 }, &mut v[range.clone()]);
                            for k in range {
                                c |= pfield(ui, pdrag(&mut v[k]).speed(if k < 3 { 0.5 } else { 0.01 })).changed();
                            }
                        });
                    }
                }
            }
            NodeKind::TwoBoneIk(set) => {
                ui.small("Bends a root → mid → tip chain (thigh, shin, foot) so the tip reaches the goal. Keyframe the goal to animate.");
                c |= joint_menu(ui, ("ik", id, 0), "Root", &mut set.root, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 1), "Mid", &mut set.mid, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 2), "Tip", &mut set.tip, &joint_names, false);
                for (label, value) in [("Goal", &mut set.goal), ("Pole", &mut set.pole)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut value[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut value[k]).speed(0.01)).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.use_pole, "Bend towards the pole").changed();
                c |= ui.checkbox(&mut set.keep_tip, "Keep the tip's orientation").changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::ProceduralMotion(set) => {
                egui::ComboBox::from_label("Motion").selected_text(kinefx::MOTIONS[set.motion.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::MOTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.motion, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.rate, 0.0..=5.0).text("Cycles per second")).changed();
                c |= pfield(ui, pslider(&mut set.amplitude, 0.0..=3.0).text("Amplitude")).changed();
                match set.motion {
                    0 => c |= pfield(ui, pslider(&mut set.travel, 0.0..=5.0).text("Forward speed")).changed(),
                    1 => {
                        c |= pfield(ui, pslider(&mut set.lag, 0.0..=3.0).text("Lag per joint")).changed();
                        ui.horizontal(|ui| {
                            ui.label("Bend about");
                            for (value, label) in sops::AXES.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                            }
                        });
                    }
                    _ => {}
                }
            }
            NodeKind::SkeletonBlend { weight } => {
                ui.small("Blends two poses of the same skeleton (input 1 at 0, input 2 at 1).");
                c |= pfield(ui, pslider(weight, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::JointCapture(set) => {
                ui.small("Input 1: mesh • Input 2: skeleton in its rest pose. Writes capture weights for Bone Deform.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=kinefx::MAX_INFLUENCES).text("Joints per point")).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.5..=12.0).text("Falloff")).on_hover_text("Higher: sharper transitions between bones").changed();
                c |= ui.checkbox(&mut set.visualize, "Colour by joint").changed();
            }
            NodeKind::BoneDeform => {
                ui.small("Input 1: captured mesh (Joint Capture) • Input 2: rest skeleton • Input 3: animated skeleton.");
            }
            NodeKind::Muscle(set) => {
                ui.small("A muscle from origin to insertion, or along a curve connected to input 1. Animate Activation to flex it.");
                for (label, point) in [("Origin", &mut set.origin), ("Insertion", &mut set.insertion)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut point[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut point[k]).speed(0.01)).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Shape").selected_text(muscle::PROFILES[set.profile.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in muscle::PROFILES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.profile, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.radius, 0.005..=2.0).logarithmic(true).text("Belly radius")).changed();
                if set.profile == 1 {
                    c |= pfield(ui, pslider(&mut set.flatness, 1.0..=10.0).text("Flatness")).changed();
                    c |= pfield(ui, pslider(&mut set.twist, -180.0..=180.0).text("Twist°")).changed();
                }
                c |= pfield(ui, pslider(&mut set.tendon, 0.0..=0.45).text("Tendon length")).changed();
                c |= pfield(ui, pslider(&mut set.activation, 0.0..=1.0).text("Activation")).changed();
                c |= pfield(ui, pslider(&mut set.contraction, 0.0..=0.8).text("Max contraction")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 2..=128).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.sides, 3..=64).text("Sides")).changed();
                c |= pfield(ui, pslider(&mut set.id, 0..=999).text("Muscle id")).changed();
            }
            NodeKind::MuscleSolver(set) => {
                ui.small("Jiggle on animated muscles (flexing or moved by their bones). The origin and insertion tips stay pinned.");
                c |= pfield(ui, pslider(&mut set.stiffness, 1.0..=2000.0).logarithmic(true).text("Firmness")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=50.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.smoothing, 0..=50).text("Coherence")).on_hover_text("Higher wobbles more as one body").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
            }
            NodeKind::MuscleSkin(set) => {
                ui.small("Input 1: skin • Input 2: muscles (animated). The skin follows how the muscles move from frame 0.");
                c |= pfield(ui, pslider(&mut set.falloff, 0.001..=5.0).logarithmic(true).text("Reach")).changed();
                c |= pfield(ui, pslider(&mut set.influences, 1..=16).text("Influences")).changed();
            }
            NodeKind::Terrain(set) => {
                ui.small("A flat heightfield grid. Shape it with Terrain Noise, Erode and Terrace.");
                ui.horizontal(|ui| {
                    if group_label(ui, "Size", 0.1, &mut set.size[..]) {
                        c = true;
                        set.size.iter_mut().for_each(|v| *v = v.clamp(0.01, 10_000.0));
                    }
                    c |= pfield(ui, pdrag(&mut set.size[0]).speed(0.1).range(0.01..=10_000.0)).changed();
                    c |= pfield(ui, pdrag(&mut set.size[1]).speed(0.1).range(0.01..=10_000.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.resolution, 2..=2048).logarithmic(true).text("Resolution")).on_hover_text("Grid cells along the longer side").changed();
            }
            NodeKind::TerrainNoise(set) => {
                egui::ComboBox::from_label("Noise").selected_text(terrain::NOISE_TYPES[set.kind.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::NOISE_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Blend").selected_text(terrain::BLEND_MODES[set.blend.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::BLEND_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.blend, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.amplitude, -10.0..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(&mut set.element_size, 0.05..=100.0).logarithmic(true).text("Element size")).on_hover_text("Size of the largest features").changed();
                c |= pfield(ui, pslider(&mut set.octaves, 1..=12).text("Octaves")).changed();
                c |= pfield(ui, pslider(&mut set.roughness, 0.0..=1.0).text("Roughness")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Offset", 0.05, &mut set.offset[..]);
                    c |= pfield(ui, pdrag(&mut set.offset[0]).speed(0.05)).changed();
                    c |= pfield(ui, pdrag(&mut set.offset[1]).speed(0.05)).changed();
                });
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                c |= ui.checkbox(&mut set.positive, "Positive only (0 to Height)").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").on_hover_text("Uses the `mask` attribute from Terrain Mask or Attribute Paint").changed();
            }
            NodeKind::TerrainErode(set) => {
                ui.small("Hydraulic erosion carves rivers and gullies; thermal erosion crumbles steep slopes. Writes `flow` and `debris` for Terrain Mask.");
                ui.label("Hydraulic");
                c |= pfield(ui, pslider(&mut set.droplets, 0..=2_000_000).logarithmic(true).text("Raindrops")).changed();
                c |= pfield(ui, pslider(&mut set.erosion, 0.0..=1.0).text("Erosion rate")).changed();
                c |= pfield(ui, pslider(&mut set.deposition, 0.0..=1.0).text("Deposition rate")).changed();
                c |= pfield(ui, pslider(&mut set.capacity, 0.0..=20.0).text("Sediment capacity")).changed();
                c |= pfield(ui, pslider(&mut set.evaporation, 0.0..=0.5).text("Evaporation")).changed();
                c |= pfield(ui, pslider(&mut set.inertia, 0.0..=0.95).text("Inertia")).on_hover_text("Higher: straighter, wider rivers").changed();
                c |= pfield(ui, pslider(&mut set.radius, 1..=8).text("Erosion radius (cells)")).changed();
                ui.label("Thermal");
                c |= pfield(ui, pslider(&mut set.thermal_iterations, 0..=500).text("Iterations")).changed();
                c |= pfield(ui, pslider(&mut set.talus_angle, 1.0..=89.0).text("Talus angle°")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::TerrainTerrace(set) => {
                c |= pfield(ui, pslider(&mut set.step, 0.01..=10.0).logarithmic(true).text("Step height")).changed();
                c |= pfield(ui, pslider(&mut set.offset, -5.0..=5.0).text("Offset")).changed();
                c |= pfield(ui, pslider(&mut set.sharpness, 0.0..=1.0).text("Sharpness")).on_hover_text("1: flat steps with sheer walls").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").changed();
            }
            NodeKind::TerrainMask(set) => {
                ui.small("Writes a 0–1 `mask` point attribute for Terrain Noise, Terrace or any other node.");
                egui::ComboBox::from_label("From").selected_text(terrain::MASK_SOURCES[set.source.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_SOURCES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.source, value, *label).changed();
                    }
                });
                let unit = match set.source {
                    0 => "height",
                    1 => "slope°",
                    _ => "value",
                };
                c |= pfield(ui, pdrag(&mut set.min).speed(0.05).prefix(format!("min {unit} "))).changed();
                c |= pfield(ui, pdrag(&mut set.max).speed(0.05).prefix(format!("max {unit} "))).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.0..=50.0).text("Falloff")).changed();
                c |= pfield(ui, pslider(&mut set.blur, 0..=100).text("Blur passes")).changed();
                egui::ComboBox::from_label("Combine with existing mask").selected_text(terrain::MASK_COMBINE[set.combine.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_COMBINE.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                c |= ui.checkbox(&mut set.visualize, "Show in viewport (Cd)").changed();
            }
            NodeKind::TerrainColorize(set) => {
                ui.small("Colours the terrain (Cd) by height and slope, for the viewport and MoonRay.");
                for (label, colour) in [("Sand", &mut set.sand), ("Grass", &mut set.grass), ("Rock", &mut set.rock), ("Snow", &mut set.snow)] {
                    ui.horizontal(|ui| {
                        c |= ui.color_edit_button_rgb(colour).changed();
                        ui.label(label);
                    });
                }
                c |= pfield(ui, pslider(&mut set.sand_level, 0.0..=1.0).text("Sand up to")).changed();
                c |= pfield(ui, pslider(&mut set.snow_level, 0.0..=1.0).text("Snow from")).changed();
                c |= pfield(ui, pslider(&mut set.rock_slope, 0.0..=90.0).text("Rock slope°")).changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=0.5).text("Blend")).changed();
            }
            NodeKind::UvTexture(set) => {
                egui::ComboBox::from_label("Projection").selected_text(sops::UV_PROJECTIONS[set.projection.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in sops::UV_PROJECTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.projection, value, *label).changed();
                    }
                });
                if matches!(set.projection, 0 | 1) {
                    ui.horizontal(|ui| {
                        ui.label(if set.projection == 0 { "Project along" } else { "Cylinder axis" });
                        for (value, label) in sops::AXES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                        }
                    });
                }
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Scale", 0.01, &mut set.scale[..]);
                    c |= pfield(ui, pdrag(&mut set.scale[0]).speed(0.01)).changed();
                    c |= pfield(ui, pdrag(&mut set.scale[1]).speed(0.01)).changed();
                    c |= group_label(ui, "Offset", 0.01, &mut set.offset[..]);
                    c |= pfield(ui, pdrag(&mut set.offset[0]).speed(0.01)).changed();
                    c |= pfield(ui, pdrag(&mut set.offset[1]).speed(0.01)).changed();
                });
                ui.small("Writes vertex:uv. Follow with UV Quick Shade to see them.");
            }
            NodeKind::UvTransform(set) => {
                for (label, value) in [("Translate", &mut set.translate), ("Scale", &mut set.scale), ("Pivot", &mut set.pivot)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut value[..]);
                        c |= pfield(ui, pdrag(&mut value[0]).speed(0.01)).changed();
                        c |= pfield(ui, pdrag(&mut value[1]).speed(0.01)).changed();
                    });
                }
                c |= pfield(ui, pslider(&mut set.rotate, -180.0..=180.0).text("Rotate")).changed();
            }
            NodeKind::UvQuickShade { file } => {
                ui.small("Shows a texture on the UVs in the viewport. Leave the file empty for a checker grid.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("checker grid").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg", "tga", "bmp", "exr", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                    if !file.is_empty() && ui.button("Clear").clicked() {
                        file.clear();
                        c = true;
                    }
                });
            }
            NodeKind::VellumSolver(set) => {
                ui.heading("Cloth Solver");
                ui.small("Input 1: geometry (a mesh for cloth/softbody, curves for hair, points for grains) • Input 2: optional static collider. Pin points with f@pin = 1; hair roots are pinned automatically.");
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for (value, label) in vellum::VELLUM_TYPES.iter().enumerate() {
                        if ui.selectable_label(set.kind == value, *label).clicked() && set.kind != value {
                            // Switching type loads that type's typical settings.
                            *set = vellum::VellumSettings::defaults(value);
                            c = true;
                        }
                    }
                });
                ui.strong("Constraints");
                if set.kind != 3 {
                    c |= pfield(ui, pslider(&mut set.stretch_stiffness, 1.0..=1e8).logarithmic(true).text("Stretch stiffness")).changed();
                }
                if matches!(set.kind, 0 | 1) {
                    c |= pfield(ui, pslider(&mut set.bend_stiffness, 1e-3..=1e5).logarithmic(true).text("Bend stiffness")).changed();
                }
                if set.kind == 2 {
                    c |= pfield(ui, pslider(&mut set.shape_stiffness, 0.0..=1.0).text("Shape stiffness")).changed();
                }
                c |= pfield(ui, pslider(&mut set.mass, 1e-3..=100.0).logarithmic(true).text("Mass per point")).changed();
                let thickness_label = if set.kind == 3 { "Grain radius (pscale overrides)" } else { "Thickness" };
                c |= pfield(ui, pslider(&mut set.thickness, 1e-3..=0.5).logarithmic(true).text(thickness_label)).changed();
                ui.strong("Forces and collisions");
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=5.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=1.0).text("Friction")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane (y = 0)").changed();
                c |= ui.checkbox(&mut set.self_collision, "Self collision").on_hover_text("Point-based: keeps points apart by their thickness").changed();
                c |= ui
                    .checkbox(&mut set.pin_to_animation, "Pins follow animation")
                    .on_hover_text("Pinned points track the animated input (e.g. hair roots on a moving head)")
                    .changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(&mut set.substeps, 1..=100).text("Substeps"))
                    .on_hover_text("Raise this first if cloth stretches: small steps converge better than extra iterations")
                    .changed();
                c |= pfield(ui, pslider(&mut set.iterations, 1..=200).text("Constraint iterations")).changed();
            }
            NodeKind::RbdSolver(set) => {
                ui.heading("RBD Bullet Solver");
                ui.small("Input 1: pieces — each connected part (or each value of a piece point attribute) is one rigid body; points become small spheres • Input 2: optional static collider.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Collision shape");
                    for (value, label) in sims::RbdSettings::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape == 1 {
                    ui.weak("Convex decomposition fits concave pieces but is slow to build.");
                }
                c |= pfield(ui, pslider(&mut set.density, 0.01..=10.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -20.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                    if set.ground {
                        c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.05).prefix("Height ")).changed();
                    }
                });
                c |= vec3_ui(ui, "Initial velocity", &mut set.velocity, 0.05);
                c |= vec3_ui(ui, "Initial spin (rad/s)", &mut set.spin, 0.05);
                ui.small("A point attribute v adds per-piece initial velocity. Output adds point attributes v and piece.");
            }
            NodeKind::SopSolver { substeps } => {
                ui.heading("SOP Solver");
                if solver_display_is_input {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "⚠ The display flag inside this solver is on Prev_Frame/Input_1, so each step changes nothing. Enter the network and set the display flag (double-click) on your last node.",
                    );
                }
                ui.label("Each step cooks this node's network with Prev_Frame set to the previous step's result, so changes accumulate over time. Input_1 is the solver's input at the current step.");
                c |= pfield(ui, pslider(substeps, 1..=16).text("Substeps per frame")).changed();
                if ui.button("Enter solver network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("At time 0 the output is the input. Example: Prev_Frame → Wrangle (@P.y += 0.02;) with the display flag on the wrangle.");
            }
            NodeKind::Subnet { values, spec } => {
                ui.heading(spec.asset.as_ref().map_or_else(|| "Subnet".to_owned(), |(name, version)| format!("{name} · asset version {version}")));
                if !spec.description.is_empty() {
                    ui.label(spec.description.as_str());
                }
                if spec.params.is_empty() {
                    ui.small("A network packaged as one node: double-click it to build inside. Inputs 1-4 come in through Subnet Input nodes; the output is the display node inside. Promote parameters of the nodes inside, below, to set them here.");
                }
                for (p, value) in spec.params.iter().zip(values.iter_mut()) {
                    let (lo, hi) = (p.min.min(p.max), p.max.max(p.min));
                    c |= pfield(ui, pslider(value, lo..=hi).clamping(egui::SliderClamping::Never).text(p.label.as_str())).changed();
                }
            }
            NodeKind::SubnetInput { index } => {
                ui.heading(format!("Subnet Input {}", *index + 1));
                ui.horizontal(|ui| {
                    for i in 0..4 {
                        c |= ui.selectable_value(index, i, format!("Input {}", i + 1)).changed();
                    }
                });
                ui.small("Inside a Subnet: what's wired into that input of the Subnet.");
            }
            NodeKind::SolverInput { index } => {
                ui.heading(if *index == 0 { "Prev_Frame" } else { "Input_1" });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(index, 0, "Prev_Frame").changed();
                    c |= ui.selectable_value(index, 1, "Input_1").changed();
                });
                ui.small("Prev_Frame is the solver's result from the previous step; Input_1 is the geometry wired into the SOP Solver, at the current step time.");
            }
            NodeKind::LopSopImport { geometry, path } => {
                ui.heading("SOP Import");
                ui.small("Brings OBJ geometry objects into the stage as Mesh prims: attributes become primvars (Cd → displayColor) and the object's MAT material is bound.");
                egui::ComboBox::from_id_salt("sop_import_geometry")
                    .selected_text(geometry.map_or("All geometry objects".to_string(), |g| format!("geometry{g}")))
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(geometry, None, "All geometry objects").changed();
                        for g in &geometry_objects {
                            c |= ui.selectable_value(geometry, Some(*g), format!("geometry{g}")).changed();
                        }
                    });
                c |= prim_path_ui(ui, if geometry.is_some() { "Prim path" } else { "Parent path" }, path);
            }
            NodeKind::LopReference { file, path, sublayer } => {
                ui.heading("Reference / Sublayer");
                if !usd::NATIVE {
                    ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "This build has no OpenUSD (native-usd feature), so USD files cannot be read.");
                }
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/asset.usd").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("USD", &["usd", "usda", "usdc", "usdz"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(sublayer, false, "Reference under a prim").changed();
                    c |= ui.selectable_value(sublayer, true, "Sublayer at the root").changed();
                });
                if !*sublayer {
                    c |= prim_path_ui(ui, "Prim path", path);
                }
                if ui.button("Reload file").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Opens .usd, .usda, .usdc and .usdz with full composition; animated files follow the timeline.");
            }
            NodeKind::LopPrimitive { shape, path, size, t } => {
                ui.heading("Primitive");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in LOP_SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(shape, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                c |= pfield(ui, pslider(size, 0.01..=10.0).logarithmic(true).text("Size (radius)")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.05);
            }
            NodeKind::LopTransform { pattern, t, r, s } => {
                ui.heading("Transform");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= vec3_ui(ui, "Translate", t, 0.05);
                c |= vec3_ui(ui, "Rotate°", r, 0.5);
                c |= vec3_ui(ui, "Scale", s, 0.02);
                ui.small("Applied in each matching prim's parent space, after its own transform.");
            }
            NodeKind::LopLight { kind, path, position, target, color, intensity, size } => {
                ui.heading("Light");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in ["Distant", "Sphere", "Dome", "Rect"].iter().enumerate() {
                        c |= ui.selectable_value(kind, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                if *kind != 2 {
                    c |= vec3_ui(ui, "Position", position, 0.05);
                    c |= vec3_ui(ui, "Aim at", target, 0.05);
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= pfield(ui, pslider(intensity, 0.0..=20.0).text("Intensity")).changed();
                let size_label = match *kind {
                    0 => Some("Angle (degrees)"),
                    1 => Some("Radius"),
                    3 => Some("Width / height"),
                    _ => None,
                };
                if let Some(label) = size_label {
                    c |= pfield(ui, pslider(size, 0.0..=20.0).text(label)).changed();
                }
            }
            NodeKind::LopCamera { path, position, target, focal, aperture } => {
                ui.heading("Camera");
                c |= prim_path_ui(ui, "Prim path", path);
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Look at", target, 0.05);
                c |= pfield(ui, pslider(focal, 8.0..=300.0).logarithmic(true).text("Focal length (mm)")).changed();
                c |= pfield(ui, pslider(aperture, 5.0..=70.0).text("Horizontal aperture (mm)")).changed();
                if ui.button("Copy from viewport").clicked() {
                    (*position, *target) = viewport_view;
                    c = true;
                }
                ui.small("Rendering the STAGE with the Auto camera uses the first stage camera.");
            }
            NodeKind::LopMaterial { pattern, material } => {
                ui.heading("Assign Material");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                let selected = material
                    .and_then(|m| material_ids.iter().find(|(id, _)| *id == m))
                    .map_or("Choose a material".to_string(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("lop_material").selected_text(selected).show_ui(ui, |ui| {
                    for (mid, name) in &material_ids {
                        c |= ui.selectable_value(material, Some(*mid), name).changed();
                    }
                });
                ui.small("MAT materials become UsdPreviewSurface materials under /materials; MoonRay renders them with the full MAT settings.");
            }
            NodeKind::LopPrune { pattern, hide } => {
                ui.heading("Prune");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= ui.checkbox(hide, "Only hide (keep the prims active)").changed();
            }
            NodeKind::LopMerge => {
                ui.heading("Merge");
                ui.label("Layers each input's prims over the inputs before it (any number of inputs: drop wires on the node).");
            }
            NodeKind::LopExport { file } => {
                ui.heading("USD Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/scene.usda").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new().set_file_name("scene.usda").add_filter("USD", &["usda", "usdc", "usd", "usdz"]).save_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Save USD")).clicked() {
                    self.usd_export_request = Some(id);
                }
                ui.small(if usd::NATIVE {
                    "The extension picks the format: .usda (text), .usdc or .usd (binary), .usdz (package)."
                } else {
                    "Saves .usda; .usdc and .usdz need the native-usd build."
                });
            }
            NodeKind::ConvertVdb { mode, iso } => {
                ui.heading("Convert VDB");
                ui.horizontal(|ui| {
                    ui.label("Convert to");
                    c |= ui.selectable_value(mode, 0, "Polygons").changed();
                    c |= ui.selectable_value(mode, 1, "Volume (fog)").changed();
                });
                if *mode == 0 {
                    c |= pfield(ui, pslider(iso, -1.0..=1.0).text("Isovalue")).changed();
                    ui.small("Level sets: distance from the surface (0 = the surface). Fog volumes and Pyro: density threshold, e.g. 0.5.");
                } else {
                    ui.small("Level sets become fog densities (1 inside); fog volumes pass through. Drawn as a soft volume.");
                }
                ui.small("Input: a VDB from VDB from Polygons, a .vdb file read with the File node, or Pyro smoke.");
            }
            NodeKind::FileImport { file, take, looping } => {
                ui.heading("File");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/model.$F4.obj").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .add_filter("Geometry", &["obj", "fbx", "abc", "stl", "usd", "usda", "usdc", "usdz", "vdb"])
                            .pick_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if fileio::has_frame_token(file) {
                    ui.weak(format!("Frame now: {}", fileio::expand_frame(file, self.time, sims::SIM_FPS)));
                }
                // FBX: which take plays, from the file's own list.
                if fileio::extension(file.trim()) == "fbx" {
                    let takes = fileio::fbx_take_names(file.trim());
                    if takes.is_empty() {
                        ui.weak("No animation takes in this file.");
                    } else {
                        let label = |name: &str, seconds: f64| format!("{name} ({seconds:.2} s)");
                        let shown = if take.is_empty() {
                            format!("First: {}", takes[0].0)
                        } else {
                            takes.iter().find(|(n, _)| n == take).map_or_else(|| format!("{take} (not in the file)"), |(n, s)| label(n, *s))
                        };
                        egui::ComboBox::from_label("Take").selected_text(shown).show_ui(ui, |ui| {
                            if ui.selectable_label(take.is_empty(), format!("First take ({})", takes[0].0)).clicked() {
                                take.clear();
                                c = true;
                            }
                            for (name, seconds) in &takes {
                                if ui.selectable_label(take == name, label(name, *seconds)).clicked() {
                                    *take = name.clone();
                                    c = true;
                                }
                            }
                        });
                        c |= ui.checkbox(looping, "Loop (else hold the last pose)").changed();
                    }
                }
                if ui.button("Reload").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Reads .obj, FBX (.fbx: the chosen take's animation, skinning and blend shapes play from frame 1), Alembic (.abc, animated: read at the current time), .stl, USD (.usd .usda .usdc .usdz) and .vdb. $F / $F4 insert the frame number for sequences. Separate objects get a piece attribute.");
            }
            NodeKind::FileExport { file } => {
                ui.heading("File Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/out.$F4.fbx").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .set_file_name("geometry.obj")
                            .add_filter("Geometry", &["obj", "fbx", "abc", "stl", "usd", "usda", "usdc", "vdb"])
                            .save_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Write current frame")).clicked() {
                        self.file_export_request = Some((id, false));
                    }
                    // Alembic keeps the whole range in one file; other formats need $F for one file per frame.
                    let can_range = fileio::has_frame_token(file) || fileio::extension(file.trim()) == "abc";
                    if ui
                        .add_enabled(can_range, egui::Button::new("Write timeline range"))
                        .on_disabled_hover_text("Add $F or $F4 to the file name to write one file per frame, or write an .abc (one animated file)")
                        .clicked()
                    {
                        self.file_export_request = Some((id, true));
                    }
                });
                ui.small("The extension picks the format: .obj, .fbx, .abc (Alembic: the timeline range goes in one animated file), .stl, .usd/.usda/.usdc or .vdb (VDB input). The geometry passes through unchanged.");
            }
            NodeKind::Merge | NodeKind::Null => {
                ui.label("No parameters.");
            }
        }

        let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
        if !targets.is_empty() {
            let animated = self.graph.is_animated(id);
            egui::CollapsingHeader::new(if animated { "Animation ◆" } else { "Animation" })
                .id_salt(("animation", id))
                .default_open(animated)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("◆ Key all").on_hover_text("Key every parameter below at this frame").clicked() {
                            for (target, _) in &targets {
                                self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
                            }
                            c = true;
                        }
                        if animated && ui.button("Remove animation").clicked() {
                            self.graph.channels.retain(|ch, _| ch.node != id);
                            c = true;
                        }
                        if ui.button("📈 Graph Editor").clicked() {
                            self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, true));
                            self.graph_editor.frame_all();
                        }
                    });
                    egui::Grid::new(("animation_grid", id)).num_columns(4).striped(true).show(ui, |ui| {
                        for (target, name) in &targets {
                            let channel_id = ChannelId { node: id, target: *target };
                            let value = self.graph.channel_value(channel_id).unwrap_or(0.0);
                            let expression = self.graph.channels.get(&channel_id).and_then(|ch| ch.expression.clone());
                            let state = self.graph.channels.get(&channel_id).map(|ch| ch.has_key_at(self.time));
                            let (symbol, hover) = match (&expression, state) {
                                (Some(_), _) => ("=", "Driven by its expression"),
                                (None, Some(true)) => ("◆", "Key at this frame: click to delete it"),
                                (None, Some(false)) => ("◇", "Animated: click to key this frame"),
                                (None, None) => ("○", "Click to animate: keys this frame"),
                            };
                            let colour = match (&expression, state) {
                                (Some(_), _) => egui::Color32::from_rgb(110, 170, 255),
                                (None, Some(true)) => egui::Color32::from_rgb(255, 200, 60),
                                (None, Some(false)) => egui::Color32::from_rgb(120, 200, 120),
                                (None, None) => egui::Color32::GRAY,
                            };
                            if ui.add_enabled(expression.is_none(), egui::Button::new(egui::RichText::new(symbol).color(colour)).small()).on_hover_text(hover).on_disabled_hover_text(hover).clicked() {
                                if state == Some(true) {
                                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                                        channel.remove_key_at(self.time);
                                        if channel.is_empty() {
                                            self.graph.channels.remove(&channel_id);
                                        }
                                    }
                                } else {
                                    self.graph.set_key(channel_id, self.time);
                                }
                                c = true;
                            }
                            ui.label(name);
                            ui.monospace(format!("{value:.3}"));
                            // An expression drives the parameter at every frame, e.g. $F for Scatter's seed.
                            let mut text = expression.clone().unwrap_or_default();
                            let error = expression.as_deref().and_then(|e| vex::eval_expression(e, self.time).err());
                            let mut edit = egui::TextEdit::singleline(&mut text).hint_text("expression").desired_width(110.0).font(egui::TextStyle::Monospace);
                            if error.is_some() {
                                edit = edit.text_color(egui::Color32::from_rgb(240, 110, 110));
                            }
                            let response = ui.add(edit).on_hover_text(error.clone().unwrap_or_else(|| {
                                "Expression (VEX): $F frame, $FF fractional frame, $T seconds, $FPS; e.g. $F, $FF * 0.1, sin($T * 2) * 0.5, rand($F) * 10".into()
                            }));
                            if response.changed() {
                                let trimmed = text.trim();
                                if trimmed.is_empty() {
                                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                                        channel.expression = None;
                                        if channel.is_empty() {
                                            self.graph.channels.remove(&channel_id);
                                        }
                                    }
                                } else {
                                    self.graph.channels.entry(channel_id).or_default().expression = Some(text.clone());
                                }
                                c = true;
                            }
                            ui.end_row();
                        }
                    });
                    ui.small("Edits to animated parameters (panel or gizmo) key the current frame automatically. An expression replaces the keys.");
                });
        }
        if self.graph.nodes.get(&id).is_some_and(|n| matches!(n.kind, NodeKind::Subnet { .. })) {
            c |= self.subnet_ui(ui, id);
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_lop()) {
            // Cook the stage first so its errors below are current.
            self.scene_graph_ui(ui, id);
        }
        if let Some(error) = node_errors(|errors| errors.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
        }
        if let Some(note) = node_notes(|notes| notes.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 190, 90), format!("⚠ {note}"));
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_solver()) {
            ui.add_space(4.0);
            if ui.button("Reset simulations").on_hover_text("Forget cached solver steps and frames, freeing their memory, and simulate again from the start").clicked() {
                sims::clear_all();
                self.frame_cache.clear();
                self.frame_cache_order.clear();
                c = true;
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if !is_object && ui.button("Set Display").clicked() {
                self.graph.set_display(id);
                c = true;
            }
            if ui.button("Duplicate").clicked() {
                self.selected = self.graph.duplicate(id).or(Some(id));
                c = true;
            }
        });
        c
    }

    // ---------------- Network editor ----------------
    fn node_editor(&mut self, ui: &mut egui::Ui) {
        let view_parent = self.view_parent();
        if self.context == AppContext::Obj || self.network().is_some() {
            let root = if self.context == AppContext::Mat { "MAT" } else { "OBJ" };
            ui.horizontal(|ui| {
                if ui.selectable_label(self.network().is_none(), root).clicked() && self.network().is_some() {
                    self.enter_network(None);
                }
                if let Some(open) = self.network() {
                    if let Some(outer) = self.parent_network() {
                        ui.label("›");
                        let outer_label = self.graph.nodes.get(&outer).map_or_else(String::new, |n| n.title(outer));
                        if ui.selectable_label(false, outer_label).clicked() {
                            self.enter_network(Some(outer));
                        }
                    }
                    ui.label("›");
                    ui.strong(self.graph.nodes.get(&open).map_or_else(String::new, |n| n.title(open)));
                    if ui.small_button("⬆ Up").on_hover_text("Back to the containing network (U)").clicked() {
                        let outer = self.parent_network();
                        self.enter_network(outer);
                    }
                }
            });
        }
        let (canvas, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        painter.rect_filled(canvas.rect, 0.0, egui::Color32::from_gray(30));
        let origin = canvas.rect.min.to_vec2() + self.graph_pan;
        if canvas.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                    let graph_point = (pointer.to_vec2() - origin) / self.graph_zoom;
                    self.graph_zoom = (self.graph_zoom * (1.0 + scroll * 0.001)).clamp(0.35, 2.5);
                    self.graph_pan = pointer.to_vec2()
                        - canvas.rect.min.to_vec2()
                        - graph_point * self.graph_zoom;
                }
            }
        }
        let mut actions: Vec<Action> = Vec::new();

        // Background dot grid
        let step = 40.0 * self.graph_zoom;
        let off = egui::vec2(self.graph_pan.x.rem_euclid(step), self.graph_pan.y.rem_euclid(step));
        let mut y = canvas.rect.top() + off.y;
        while y < canvas.rect.bottom() {
            let mut x = canvas.rect.left() + off.x;
            while x < canvas.rect.right() {
                painter.circle_filled(egui::pos2(x, y), 1.0, egui::Color32::from_gray(55));
                x += step;
            }
            y += step;
        }

        // Shift-drag on empty space draws a box that selects the nodes it touches.
        let shift = ui.input(|i| i.modifiers.shift);
        if canvas.drag_started_by(egui::PointerButton::Primary) && shift {
            self.node_box = canvas.interact_pointer_pos();
        }
        if canvas.dragged_by(egui::PointerButton::Middle)
            || (canvas.dragged_by(egui::PointerButton::Primary) && self.dragging_link.is_none() && self.node_box.is_none())
        {
            self.graph_pan += canvas.drag_delta();
        }
        if let (Some(start), Some(now)) = (self.node_box, ui.input(|i| i.pointer.interact_pos())) {
            let area = egui::Rect::from_two_pos(start, now);
            painter.rect(area, 0.0, egui::Color32::from_rgba_unmultiplied(255, 200, 60, 20), egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 200, 60)), egui::StrokeKind::Inside);
            if canvas.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.node_box = None;
                let mut inside: Vec<NodeId> = self.graph.nodes.iter()
                    .filter(|(_, n)| n.context == self.context && n.parent == view_parent && node_rect(n.pos, origin, self.graph_zoom).intersects(area))
                    .map(|(id, _)| *id)
                    .collect();
                inside.sort_unstable();
                // The box adds to what's already selected.
                let mut chosen = self.selected_nodes();
                for id in inside {
                    if !chosen.contains(&id) {
                        chosen.push(id);
                    }
                }
                self.selected = chosen.last().copied();
                self.node_selection = chosen;
            }
        }
        if canvas.clicked() && !shift {
            self.selected = None;
            self.node_selection.clear();
        }
        let chosen: HashSet<NodeId> = self.selected_nodes().into_iter().collect();
        if canvas.secondary_clicked() {
            if let Some(p) = canvas.interact_pointer_pos() {
                self.menu_pos = p;
            }
            self.menu_query.clear();
        }
        let menu_pos = self.menu_pos;
        let templates = self.templates_for_view();
        let zoom = self.graph_zoom;
        let mut query = std::mem::take(&mut self.menu_query);
        let mut pick = self.search_pick;
        canvas.clone().context_menu(|ui| {
            let at = egui::pos2((menu_pos.x - origin.x) / zoom, (menu_pos.y - origin.y) / zoom);
            if let Some(kind) = node_search_ui(ui, &mut query, &mut pick, &templates) {
                actions.push(Action::Add(kind, at));
                ui.close_menu();
            }
        });
        self.menu_query = query;
        self.search_pick = pick;

        // Wires (only within current context)
        for l in &self.graph.links {
            if let (Some(a), Some(b)) = (self.graph.nodes.get(&l.from), self.graph.nodes.get(&l.to)) {
                if a.context != self.context || b.context != self.context { continue; }
                if a.parent != view_parent || b.parent != view_parent { continue; }
                let p0 = out_port_n(node_rect(a.pos, origin, self.graph_zoom), l.out, a.kind.num_outputs());
                let p1 = in_port(node_rect(b.pos, origin, self.graph_zoom), l.slot, self.graph.input_slots(l.to));
                draw_wire(&painter, p0, p1, egui::Color32::from_gray(170));
            }
        }

        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let mut new_link: Option<((NodeId, usize), NodeId, usize)> = None;
        let mut disconnects: Vec<(NodeId, usize)> = Vec::new();

        let mut ids: Vec<NodeId> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| n.context == self.context && n.parent == view_parent)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();

        for id in ids {
            let rect = node_rect(self.graph.nodes[&id].pos, origin, self.graph_zoom);
            let resp = ui.interact(rect, ui.id().with(("node", id)), egui::Sense::click_and_drag());
            let resp = match self.node_stats.get(&id) {
                Some((p, t)) => resp.on_hover_text(format!("{p} points, {t} triangles")),
                None => resp.on_hover_text("Not cooked (not upstream of display/template)"),
            };
            if resp.dragged_by(egui::PointerButton::Primary) {
                // Dragging one of several selected nodes moves them all.
                let moved: Vec<NodeId> = if chosen.len() > 1 && chosen.contains(&id) { chosen.iter().copied().collect() } else { vec![id] };
                for m in moved {
                    if let Some(n) = self.graph.nodes.get_mut(&m) {
                        n.pos += resp.drag_delta() / self.graph_zoom;
                    }
                }
            }
            if resp.clicked() && ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                // Shift- or Ctrl-click adds a node to the selection, or takes it out.
                let mut picked = self.selected_nodes();
                if let Some(at) = picked.iter().position(|&p| p == id) {
                    picked.remove(at);
                    self.selected = picked.last().copied();
                } else {
                    picked.push(id);
                    self.selected = Some(id);
                }
                self.node_selection = picked;
            } else if resp.clicked() || (resp.drag_started() && !chosen.contains(&id)) {
                self.selected = Some(id);
                self.node_selection = vec![id];
            } else if resp.drag_started() {
                // Picking up one of the selected nodes keeps the rest selected.
                self.selected = Some(id);
            }
            // Holding the middle button on a node shows its info, like Houdini's.
            if resp.hovered() && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle)) {
                self.info_hold = ui.input(|i| i.pointer.interact_pos()).map(|p| (id, p));
            }
            if resp.double_clicked() {
                if self.graph.nodes[&id].kind.is_container() {
                    actions.push(Action::Enter(id));
                } else {
                    actions.push(Action::Display(id));
                }
            }
            let is_object = self.graph.nodes[&id].kind.is_object();
            let is_geometry = self.graph.nodes[&id].kind.is_container();
            resp.clone().context_menu(|ui| {
                if is_geometry && ui.button("Enter (double-click)").clicked() {
                    actions.push(Action::Enter(id));
                    ui.close_menu();
                }
                if ui.button("Rename (F2)").clicked() {
                    self.selected = Some(id);
                    self.focus_rename = true;
                    ui.close_menu();
                }
                if ui.button("Node Info (hold middle button)").clicked() {
                    self.inspect_node = Some(id);
                    ui.close_menu();
                }
                let display_label = if is_object { "Toggle Active / Activate Camera" } else { "Set Display Flag" };
                if ui.button(display_label).clicked() {
                    actions.push(Action::Display(id));
                    ui.close_menu();
                }
                if !is_object && ui.button("Toggle Bypass (B)").clicked() {
                    actions.push(Action::Bypass(id));
                    ui.close_menu();
                }
                let together = chosen.len() > 1 && chosen.contains(&id);
                if ui.button(if together { "Duplicate selected (Ctrl+D)" } else { "Duplicate (Ctrl+D)" }).clicked() {
                    actions.push(if together { Action::DuplicateMany(chosen.iter().copied().collect()) } else { Action::Duplicate(id) });
                    ui.close_menu();
                }
                if ui.button("Delete").clicked() {
                    actions.push(Action::Delete(id));
                    ui.close_menu();
                }
            });

            let node = &self.graph.nodes[&id];
            let rect = node_rect(node.pos, origin, self.graph_zoom);
            let n_in = self.graph.input_slots(id);

            let n_out = node.kind.num_outputs();
            let (input_names, output_names) = node.kind.port_names();
            for out in 0..n_out {
                let op = out_port_n(rect, out, n_out);
                let op_resp = ui.interact(
                    egui::Rect::from_center_size(op, egui::vec2(16.0, 16.0)),
                    ui.id().with(("out", id, out)),
                    egui::Sense::drag(),
                );
                let op_resp = match output_names.get(out) {
                    Some(name) => op_resp.on_hover_text(format!("Output: {name}")),
                    None => op_resp,
                };
                if op_resp.drag_started() {
                    self.dragging_link = Some((id, out));
                }
            }

            for slot in 0..n_in {
                let ip = in_port(rect, slot, n_in);
                let ip_resp = ui.interact(
                    egui::Rect::from_center_size(ip, egui::vec2(16.0, 16.0)),
                    ui.id().with(("in", id, slot)),
                    egui::Sense::click(),
                );
                let ip_resp = match &self.graph.nodes[&id].kind {
                    NodeKind::Vop(v) => match v.def().inputs.get(slot) {
                        Some((input_name, default)) => ip_resp.on_hover_text(format!("{input_name} (unconnected: {default})")),
                        None => ip_resp,
                    },
                    NodeKind::ShaderNode(n) => match n.def().inputs.get(slot) {
                        Some((input_name, ..)) => ip_resp.on_hover_text(*input_name),
                        None => ip_resp,
                    },
                    _ => ip_resp,
                };
                if ip_resp.clicked() {
                    disconnects.push((id, slot));
                }
                if let Some(name) = input_names.get(slot) {
                    let _ = ui.interact(egui::Rect::from_center_size(ip, egui::vec2(16.0, 16.0)), ui.id().with(("in-name", id, slot)), egui::Sense::hover()).on_hover_text(format!("Input: {name}"));
                }
                if let (Some(src), Some(p)) = (self.dragging_link, pointer) {
                    if released && p.distance(ip) < 16.0 {
                        // On a Merge every drop adds an input (as in Houdini), even on a used port.
                        let slot = if node.kind.is_multi_input() { self.graph.drop_slot(id).unwrap_or(slot) } else { slot };
                        new_link = Some((src, id, slot));
                    }
                }
            }
            // A wire dropped on the node itself (not a port): a new Merge input, else the first free input.
            if let (Some(src), Some(p), None) = (self.dragging_link, pointer, new_link) {
                if released && rect.contains(p) && src.0 != id {
                    if let Some(slot) = self.graph.drop_slot(id) {
                        new_link = Some((src, id, slot));
                    }
                }
            }

            // Draw
            let sel = chosen.contains(&id);
            let disp = if node.kind.is_object() {
                match node.kind {
                    NodeKind::Camera { .. } => self.active_camera == Some(id),
                    _ => !node.bypass,
                }
            } else {
                self.graph.is_display(id)
            };
            let body = if node.bypass {
                egui::Color32::from_rgb(90, 80, 30)
            } else {
                egui::Color32::from_gray(58)
            };
            painter.rect(
                rect,
                egui::CornerRadius::same(6),
                body,
                egui::Stroke::new(
                    if sel { 2.0 } else { 1.0 },
                    if sel { egui::Color32::from_rgb(255, 200, 60) } else { egui::Color32::from_gray(95) },
                ),
                egui::StrokeKind::Outside,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(8.0, rect.height())),
                egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                node.kind.color(),
            );
            if disp {
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.right_top() - egui::vec2(10.0, 0.0), egui::vec2(10.0, rect.height())),
                    egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 },
                    egui::Color32::from_rgb(60, 140, 255),
                );
            }
            if self.template == Some(id) {
                painter.rect_stroke(
                    rect.shrink(3.0),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 80, 200)),
                    egui::StrokeKind::Outside,
                );
            }
            if self.graph.time_dependent(id) {
                painter.circle_filled(
                    rect.left_top() + egui::vec2(16.0, 8.0),
                    3.0,
                    egui::Color32::from_rgb(120, 230, 120),
                );
            }
            if let Some((color, ..)) = node.kind.is_material().then(|| self.graph.material_preview(id)).flatten() {
                let swatch = egui::Color32::from_rgb(
                    (color[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[2].clamp(0.0, 1.0) * 255.0) as u8,
                );
                painter.circle_filled(rect.right_center() - egui::vec2(14.0 * self.graph_zoom, 0.0), 6.0 * self.graph_zoom, swatch);
            }
            let label = format!("{}{}", node.title(id), if node.bypass { " (byp)" } else { "" });
            let renamed = !node.name.trim().is_empty();
            painter.text(
                if renamed { rect.center() - egui::vec2(0.0, 6.0 * self.graph_zoom) } else { rect.center() },
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            // A renamed node still says what it is.
            if renamed {
                painter.text(
                    rect.center() + egui::vec2(0.0, 9.0 * self.graph_zoom),
                    egui::Align2::CENTER_CENTER,
                    node.type_label(),
                    egui::FontId::proportional(10.5 * self.graph_zoom.max(0.5)),
                    egui::Color32::from_gray(185),
                );
            }
            // Performance monitor: the node's latest own cook time, warmer the slower it is.
            if self.show_cook_times {
                if let Some(stat) = perf::get(id) {
                    let ms = stat.last * 1000.0;
                    let colour = if ms >= 100.0 {
                        egui::Color32::from_rgb(240, 110, 110)
                    } else if ms >= 10.0 {
                        egui::Color32::from_rgb(240, 190, 90)
                    } else {
                        egui::Color32::from_gray(170)
                    };
                    painter.text(
                        rect.right_center() + egui::vec2(8.0 * self.graph_zoom, 0.0),
                        egui::Align2::LEFT_CENTER,
                        perf::ms(stat.last),
                        egui::FontId::proportional(11.0 * self.graph_zoom.max(0.6)),
                        colour,
                    );
                }
            }
            // TOP nodes: a dot per work item from the last cook, like Houdini's.
            if let Some(states) = self.top_results.get(&id).filter(|_| matches!(node.kind, NodeKind::Top(_))) {
                let r = 3.0 * self.graph_zoom;
                let per_row = ((rect.width() / (r * 2.6)).floor() as usize).max(1);
                for (i, (state, _)) in states.iter().take(per_row * 2).enumerate() {
                    let (col, row) = (i % per_row, i / per_row);
                    let p = egui::pos2(rect.left() + r * 1.6 + col as f32 * r * 2.6, rect.bottom() + r * 2.0 + row as f32 * r * 2.6);
                    painter.circle_filled(p, r, top_state_colour(*state));
                }
                if states.len() > per_row * 2 {
                    painter.text(egui::pos2(rect.right(), rect.bottom() + r * 4.6), egui::Align2::RIGHT_CENTER, format!("+{}", states.len() - per_row * 2), egui::FontId::proportional(10.0 * self.graph_zoom), egui::Color32::from_gray(190));
                }
            }
            if node.kind.is_multi_input() {
                // A Merge takes any number of inputs, as in Houdini: one input bar across the
                // top, a dot per connected input and a "+" where the next one goes. It lights
                // up while a wire is dragged over the node; a wire dropped anywhere adds an input.
                let hot = self.dragging_link.is_some_and(|(src, _)| src != id) && pointer.is_some_and(|p| rect.expand(10.0).contains(p));
                let bar = egui::Rect::from_min_max(egui::pos2(rect.left() + 6.0, rect.top() - 3.5), egui::pos2(rect.right() - 6.0, rect.top() + 3.5));
                painter.rect_filled(bar, egui::CornerRadius::same(4), if hot { egui::Color32::YELLOW } else { egui::Color32::from_gray(150) });
                let connected: HashSet<usize> = self.graph.links.iter().filter(|l| l.to == id).map(|l| l.slot).collect();
                for slot in 0..n_in {
                    let ip = in_port(rect, slot, n_in);
                    if connected.contains(&slot) {
                        painter.circle_filled(ip, 4.5, egui::Color32::from_gray(235));
                    } else {
                        painter.text(ip, egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(11.0 * self.graph_zoom.max(0.6)), egui::Color32::from_gray(40));
                    }
                }
                let _ = ui
                    .interact(bar.expand2(egui::vec2(0.0, 5.0)), ui.id().with(("multi-input", id)), egui::Sense::hover())
                    .on_hover_text("Any number of inputs: drop wires on the bar or anywhere on the node to add one; click a wire's dot to disconnect it");
            } else {
                for slot in 0..n_in {
                    let ip = in_port(rect, slot, n_in);
                    let hot = self.dragging_link.is_some() && pointer.map_or(false, |p| p.distance(ip) < 16.0);
                    painter.circle_filled(
                        ip,
                        if hot { 7.0 } else { 5.0 },
                        if hot { egui::Color32::YELLOW } else { egui::Color32::from_gray(200) },
                    );
                }
            }
            let n_out = node.kind.num_outputs();
            for out in 0..n_out {
                painter.circle_filled(out_port_n(rect, out, n_out), 5.0, egui::Color32::from_gray(200));
            }
        }

        for (id, slot) in disconnects {
            self.graph.disconnect(id, slot);
            self.dirty = true;
        }

        if let (Some((src, out)), Some(p)) = (self.dragging_link, pointer) {
            if let Some(n) = self.graph.nodes.get(&src) {
                draw_wire(
                    &painter,
                    out_port_n(node_rect(n.pos, origin, self.graph_zoom), out, n.kind.num_outputs()),
                    p,
                    egui::Color32::YELLOW,
                );
            }
        }
        if released {
            if let Some(((a, out), b, s)) = new_link {
                if self.graph.connect_out(a, out, b, s) {
                    self.dirty = true;
                } else {
                    self.status = "Connection refused: would create a cycle".into();
                }
            }
            self.dragging_link = None;
        }

        let pointer_in = ui.rect_contains_pointer(canvas.rect);
        let typing = ui.ctx().wants_keyboard_input();

        // U: leave the open geometry
        if pointer_in && !typing && self.network().is_some() && ui.input(|i| i.key_pressed(egui::Key::U)) {
            let outer = self.parent_network();
            self.enter_network(outer);
            return;
        }

        // H: home the network view
        if pointer_in && !typing && ui.input(|i| i.key_pressed(egui::Key::H)) {
            if let Some(min) = self.graph.nodes.values().map(|n| n.pos).reduce(|a, b| a.min(b)) {
                self.graph_pan = egui::vec2(20.0, 20.0) - min.to_vec2() * self.graph_zoom;
            }
        }

        // F2: rename the selected node (in the parameters pane).
        if pointer_in && !typing && self.selected.is_some() && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.focus_rename = true;
        }
        // TAB over the network: the searchable add-node menu (auto-wires below the selected
        // node). It opens even while a field elsewhere has focus, except a VEX editor
        // (where Tab indents).
        let in_code_editor = {
            let focused = ui.ctx().memory(|m| m.focused());
            focused.is_some() && focused == ui.ctx().data(|d| d.get_temp::<egui::Id>(egui::Id::new(CODE_EDITOR_FOCUS)))
        };
        if pointer_in && !self.tab_open && !in_code_editor && ui.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.tab_open = true;
            self.tab_query.clear();
            self.search_pick = 0;
            self.tab_pos = pointer.unwrap_or(canvas.rect.center());
        }
        if self.tab_open {
            let templates = self.templates_for_view();
            let selected = self.selected.filter(|id| self.graph.nodes.contains_key(id));
            let tab_pos = self.tab_pos;
            let mut close = false;
            let (mut query, mut pick) = (std::mem::take(&mut self.tab_query), self.search_pick);
            egui::Area::new(egui::Id::new("tab_menu"))
                .fixed_pos(tab_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(220.0);
                        if let Some(kind) = node_search_ui(ui, &mut query, &mut pick, &templates) {
                            actions.push(match selected {
                                Some(from) => Action::AddBelow(kind, from),
                                None => Action::Add(kind, tab_pos - origin),
                            });
                            close = true;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                    });
                });
            self.tab_query = query;
            self.search_pick = pick;
            if close {
                self.tab_open = false;
            }
        }

        for a in actions {
            self.apply(a);
        }
    }

    // ---------------- 3D viewport ----------------
    fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        // The flipbook captures this rect of the main window (not a popped-out viewport).
        if ui.ctx().viewport_id() == egui::ViewportId::ROOT {
            self.viewport_rect = Some(response.rect);
        }
        // A flipbook shows the scene only: no info text, axes or handles.
        let recording = self.flipbook_recording.is_some();
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_gray(self.viewport_bg));
        // IMG: the viewport shows the composite.
        if self.context == AppContext::Img {
            let _ = ctx;
            self.image_view(ui, &response, &painter.with_clip_rect(response.rect));
            return;
        }

        // Restrict painter rendering strictly to the viewport rectangle
        let painter = painter.with_clip_rect(response.rect);

        let box_select_modifier = ctx.input(|i| i.modifiers.shift);
        // Gizmo handles are hit-tested against where they were drawn last frame.
        if response.drag_started_by(egui::PointerButton::Primary) && !box_select_modifier {
            if let (Some(frame), Some(pointer)) = (self.gizmo_frame.clone(), response.interact_pointer_pos()) {
                if let (Some(part), Some(target)) = (frame.hit(pointer).cloned(), self.gizmo_target()) {
                    if target.node == frame.node {
                        self.gizmo_drag = Some(GizmoDrag { part, frame, start_pointer: pointer, target });
                    }
                }
            }
        }
        if let Some(drag) = self.gizmo_drag.clone() {
            if let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) {
                self.apply_gizmo_drag(&drag, pointer);
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.gizmo_drag = None;
            }
        }
        if response.hovered() && !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::W) {
                    self.gizmo_mode = GizmoMode::Translate;
                } else if i.key_pressed(egui::Key::E) {
                    self.gizmo_mode = GizmoMode::Rotate;
                } else if i.key_pressed(egui::Key::R) {
                    self.gizmo_mode = GizmoMode::Scale;
                }
            });
        }
        let paint_node = self
            .selected
            .filter(|id| self.paint_active && matches!(self.graph.nodes.get(id).map(|n| &n.kind), Some(NodeKind::AttributePaint { .. })));
        if response.dragged_by(egui::PointerButton::Primary) && !box_select_modifier && self.gizmo_drag.is_none() && paint_node.is_none() {
            self.cam_rot_y += response.drag_delta().x * 0.01;
            self.cam_rot_x = (self.cam_rot_x + response.drag_delta().y * 0.01).clamp(-1.55, 1.55);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.cam_pan_x += response.drag_delta().x;
            self.cam_pan_y += response.drag_delta().y;
        }
        if response.hovered() {
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.cam_zoom = (self.cam_zoom * (1.0 - scroll * 0.002)).clamp(MIN_ZOOM, 200.0);
            }
        }
        self.sync_active_camera_from_viewport();

        let center = response.rect.center() + egui::vec2(self.cam_pan_x, self.cam_pan_y);
        let (sin_x, cos_x) = self.cam_rot_x.sin_cos();
        let (sin_y, cos_y) = self.cam_rot_y.sin_cos();
        let focal = response.rect.height().min(response.rect.width()) * self.camera_focal;
        self.viewport_focal_px = focal;
        let zoom = self.cam_zoom;
        let ortho_scale = focal / zoom.max(MIN_ZOOM);
        let target = self.cam_target;

        // Close in, the near clip comes closer too, or it would cut away what's in view.
        let near_plane = self.camera_near.min(self.cam_zoom * 0.1);
        let half_w = response.rect.width() * 0.5;
        let half_h = response.rect.height() * 0.5;
        let hw_focal = half_w / focal;
        let hh_focal = half_h / focal;
        let ortho_half_w = half_w / ortho_scale;
        let ortho_half_h = half_h / ortho_scale;
        let rect_min = response.rect.min;
        let rect_max = response.rect.max;
        // HDRI background: the environment along each view ray, on a coarse grid.
        if let Some((file, scale, rotation, true)) = self.environment_light() {
            if let Ok(map) = envlight::load(&file) {
                let view_dir = |pos: egui::Pos2| -> Vec3 {
                    let (rx, ry, rrz) = if self.orthographic {
                        (0.0, 0.0, 1.0)
                    } else {
                        ((pos.x - center.x) / focal, -(pos.y - center.y) / focal, 1.0)
                    };
                    // Undo the viewport rotation (inverse of to_cam).
                    let y = ry * cos_x + rrz * sin_x;
                    let rz = -ry * sin_x + rrz * cos_x;
                    Vec3::new(rx * cos_y + rz * sin_y, y, -rx * sin_y + rz * cos_y).normalize()
                };
                let tone = |v: f32| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8;
                let cell = 16.0;
                let (cols, rows) = ((response.rect.width() / cell).ceil() as usize + 1, (response.rect.height() / cell).ceil() as usize + 1);
                let mut sky = egui::epaint::Mesh::default();
                for j in 0..rows {
                    for i in 0..cols {
                        let pos = egui::pos2(rect_min.x + i as f32 * cell, rect_min.y + j as f32 * cell);
                        let l = map.radiance(view_dir(pos), rotation);
                        sky.colored_vertex(pos, egui::Color32::from_rgb(tone(l[0] * scale[0]), tone(l[1] * scale[1]), tone(l[2] * scale[2])));
                    }
                }
                for j in 0..rows - 1 {
                    for i in 0..cols - 1 {
                        let a = (j * cols + i) as u32;
                        sky.add_triangle(a, a + 1, a + cols as u32);
                        sky.add_triangle(a + 1, a + cols as u32 + 1, a + cols as u32);
                    }
                }
                painter.add(egui::Shape::mesh(sky));
            }
        }
        let gizmo_center = response.rect.right_top() + egui::vec2(-54.0, 54.0);
        let gizmo_clicked = response.clicked()
            && response.interact_pointer_pos().is_some_and(|p| {
                p.distance(gizmo_center) <= 34.0
            });
        if gizmo_clicked {
            if let Some(pointer) = response.interact_pointer_pos() {
                let delta = pointer - gizmo_center;
                if delta.x.abs() > delta.y.abs() {
                    self.set_camera_view(0.0, if delta.x < 0.0 {
                        -std::f32::consts::FRAC_PI_2
                    } else {
                        std::f32::consts::FRAC_PI_2
                    });
                } else {
                    self.set_camera_view(
                        if delta.y < 0.0 { -1.52 } else { 1.52 },
                        0.0,
                    );
                }
            }
        }
        if !recording {
        painter.circle_filled(gizmo_center, 30.0, egui::Color32::from_black_alpha(150));
        painter.circle_stroke(
            gizmo_center,
            30.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
        );
        painter.text(
            gizmo_center + egui::vec2(0.0, -13.0),
            egui::Align2::CENTER_CENTER,
            "Y",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(90, 220, 110),
        );
        painter.text(
            gizmo_center + egui::vec2(17.0, 5.0),
            egui::Align2::CENTER_CENTER,
            "X",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(230, 90, 90),
        );
        painter.text(
            gizmo_center + egui::vec2(-17.0, 5.0),
            egui::Align2::CENTER_CENTER,
            "Z",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(90, 140, 240),
        );
        }

        // Transform world coordinate into 3D camera space (+Z in front of camera)
        let to_cam = |p: Vec3| -> Vec3 {
            let v = p.sub(target);
            let rx = v.x * cos_y - v.z * sin_y;
            let rz = v.x * sin_y + v.z * cos_y;
            let ry = v.y * cos_x - rz * sin_x;
            let rrz = v.y * sin_x + rz * cos_x;
            Vec3::new(rx, ry, rrz + zoom)
        };

        // Project 3D camera-space coordinate to 2D screen coordinate
        let project_cam = |c: Vec3| -> egui::Pos2 {
            if self.orthographic {
                egui::pos2(center.x + c.x * ortho_scale, center.y - c.y * ortho_scale)
            } else {
                egui::pos2(center.x + c.x / c.z * focal, center.y - c.y / c.z * focal)
            }
        };

        // Attribute Paint brush: dabs while dragging, applied to the node after drawing.
        let mut pending_paint: Option<(NodeId, Vec<f32>)> = None;
        if let (Some(id), Some(hover)) = (paint_node, response.hover_pos()) {
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
                painter.circle_stroke(hover, settings.radius, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 210, 90)));
                if response.dragged_by(egui::PointerButton::Primary) || response.clicked() {
                    // Paint on the node's own points, placed where the viewport shows them.
                    let mut points = self.graph.cook(id, self.time, &mut HashMap::new());
                    if let Some(object) = self.graph.enclosing_geometry(id) {
                        points = self.graph.xform_at(object, self.time).apply(points);
                    }
                    let facing = paint::facing(&points, to_cam);
                    let screen: Vec<Option<(f32, f32)>> = points
                        .vertices
                        .iter()
                        .zip(&facing)
                        .map(|(p, front)| {
                            let c = to_cam(*p);
                            (*front && c.z > near_plane).then(|| project_cam(c)).map(|s| (s.x, s.y))
                        })
                        .collect();
                    let mut brush = settings.clone();
                    if ctx.input(|i| i.modifiers.command) {
                        brush.mode = 1;
                    }
                    let mut updated = paint::values_for(values, points.vertices.len(), settings.default);
                    if paint::dab(&mut updated, &points, &screen, (hover.x, hover.y), &brush) {
                        pending_paint = Some((id, updated));
                    }
                }
            }
        }

        // 3D camera-space frustum line clipper
        let clip_line_frustum_3d = |mut p1: Vec3, mut p2: Vec3| -> Option<(Vec3, Vec3)> {
            let plane_dist = |p: Vec3| p.z - near_plane;

            // Side clipping is performed after projection in screen space.
            // Applying camera-centered side planes here incorrectly clips
            // geometry after the viewport has been panned.
            for _ in 0..1 {
                let d1 = plane_dist(p1);
                let d2 = plane_dist(p2);

                if d1 < 0.0 && d2 < 0.0 {
                    return None; // Completely outside this frustum plane
                }
                if d1 < 0.0 {
                    let denom = d1 - d2;
                    if denom.abs() < 1e-7 { return None; }
                    let t = d1 / denom;
                    p1 = Vec3::new(
                        p1.x + t * (p2.x - p1.x),
                        p1.y + t * (p2.y - p1.y),
                        p1.z + t * (p2.z - p1.z),
                    );
                } else if d2 < 0.0 {
                    let denom = d2 - d1;
                    if denom.abs() < 1e-7 { return None; }
                    let t = d2 / denom;
                    p2 = Vec3::new(
                        p2.x + t * (p1.x - p2.x),
                        p2.y + t * (p1.y - p2.y),
                        p2.z + t * (p1.z - p2.z),
                    );
                }
            }

            Some((p1, p2))
        };

        // 2D Liang-Barsky line clipper against visible viewport bounds
        let clip_line_2d = |p1: egui::Pos2, p2: egui::Pos2| -> Option<(egui::Pos2, egui::Pos2)> {
            let dx = p2.x - p1.x;
            let dy = p2.y - p1.y;
            let p = [-dx, dx, -dy, dy];
            let q = [p1.x - rect_min.x, rect_max.x - p1.x, p1.y - rect_min.y, rect_max.y - p1.y];
            let mut u1 = 0.0f32;
            let mut u2 = 1.0f32;

            for i in 0..4 {
                if p[i].abs() < 1e-9 {
                    if q[i] < 0.0 {
                        return None;
                    }
                } else {
                    let t = q[i] / p[i];
                    if p[i] < 0.0 {
                        if t > u1 { u1 = t; }
                    } else if t < u2 {
                        u2 = t;
                    }
                }
            }

            if u1 > u2 {
                return None;
            }
            Some((
                egui::pos2(p1.x + u1 * dx, p1.y + u1 * dy),
                egui::pos2(p1.x + u2 * dx, p1.y + u2 * dy),
            ))
        };

        // Safe 3D line renderer with both 3D frustum & 2D viewport clipping
        let line3d = |a: Vec3, b: Vec3, stroke: egui::Stroke| {
            let c1 = to_cam(a);
            let c2 = to_cam(b);

            if let Some((cp1, cp2)) = clip_line_frustum_3d(c1, c2) {
                let p1 = project_cam(cp1);
                let p2 = project_cam(cp2);
                if let Some((cp1_2d, cp2_2d)) = clip_line_2d(p1, p2) {
                    painter.line_segment([cp1_2d, cp2_2d], stroke);
                }
            }
        };

        // Clip filled triangles in camera space before perspective projection.
        // Projecting an unclipped triangle can otherwise create long slivers
        // when it crosses the near plane or viewport edge.
        let clip_triangle = |vertices: [Vec3; 3]| -> Vec<Vec3> {
            let plane_dist = |i: usize, p: Vec3| -> f32 {
                p.z - near_plane
            };

            // In front of the near plane: unchanged, corners in their own order (the
            // loop below starts from the second corner, which would misplace corner
            // colours, uvs and polygon edges).
            if vertices.iter().all(|v| plane_dist(0, *v) >= 0.0) {
                return vertices.to_vec();
            }
            let mut polygon = vertices.to_vec();
            // Keep only near-plane clipping in camera space. The viewport
            // rectangle clips the projected polygon and remains correct
            // when the camera is panned.
            for plane in 0..1 {
                if polygon.is_empty() {
                    break;
                }
                let mut clipped = Vec::with_capacity(polygon.len() + 1);
                for i in 0..polygon.len() {
                    let a = polygon[i];
                    let b = polygon[(i + 1) % polygon.len()];
                    let da = plane_dist(plane, a);
                    let db = plane_dist(plane, b);
                    let a_inside = da >= 0.0;
                    let b_inside = db >= 0.0;

                    if a_inside != b_inside {
                        let denom = da - db;
                        if denom.abs() > 1e-7 {
                            let t = da / denom;
                            clipped.push(Vec3::new(
                                a.x + t * (b.x - a.x),
                                a.y + t * (b.y - a.y),
                                a.z + t * (b.z - a.z),
                            ));
                        }
                    }
                    if b_inside {
                        clipped.push(b);
                    }
                }
                polygon = clipped;
            }
            polygon
        };

        // Floor grid + axes
        if self.show_floor {
            let gs = egui::Stroke::new(1.0, egui::Color32::from_gray(62));
            let steps = (self.grid_extent / self.grid_spacing).round() as i32;
            for i in -steps..=steps {
                let s = i as f32 * self.grid_spacing;
                line3d(
                    Vec3::new(s, 0.0, -self.grid_extent),
                    Vec3::new(s, 0.0, self.grid_extent),
                    gs,
                );
                line3d(
                    Vec3::new(-self.grid_extent, 0.0, s),
                    Vec3::new(self.grid_extent, 0.0, s),
                    gs,
                );
            }
            let o = Vec3::zero();
            line3d(o, Vec3::new(1.0, 0.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 70, 70)));
            line3d(o, Vec3::new(0.0, 1.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 200, 70)));
            line3d(o, Vec3::new(0.0, 0.0, 1.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 120, 230)));
        }

        // Template geometry ghosted wireframe
        if self.template.is_some() && self.template_mesh.indices.len() <= 20_000 {
            let ts = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(230, 80, 200, 140));
            let tm = &self.template_mesh;
            for (t, shown) in tm.indices.iter().zip(poly::outline_edges(tm)) {
                for k in (0..3).filter(|k| shown[*k]) {
                    line3d(tm.vertices[t[k]], tm.vertices[t[(k + 1) % 3]], ts);
                }
            }
        }

        if let Some(pick) = &self.group_pick {
            let (count, what) = match pick.domain {
                groups::GroupDomain::Points => (self.selected_vertices.len(), "points"),
                groups::GroupDomain::Prims => (self.selected_triangles.len(), "primitives"),
            };
            let name = self.graph.nodes.get(&pick.node).map_or(String::new(), |n| n.title(pick.node));
            painter.text(
                response.rect.center_top() + egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_TOP,
                format!("Group for {name}: {count} {what} selected — Shift-drag to box select · Enter to accept · Esc to cancel"),
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(255, 210, 90),
            );
        }
        let mesh = &self.current_mesh;
        let projected: Vec<Option<(egui::Pos2, f32)>> = mesh
            .vertices
            .iter()
            .map(|v| {
                let c = to_cam(*v);
                (c.z >= near_plane).then(|| (project_cam(c), c.z))
            })
            .collect();
        // Polygon diagonals: hidden in the wireframe, so not pickable as edges either.
        let diagonals: HashSet<(usize, usize)> = mesh.indices.iter().zip(poly::outline_edges(mesh)).flat_map(|(t, shown)| (0..3).filter(move |k| !shown[*k]).map(move |k| (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])))).collect();
        let mut pending_edge_loop = None;
        let mut clear_selection_on_miss = false;
        let box_selecting = ctx.input(|i| i.modifiers.shift);
        if box_selecting && response.drag_started_by(egui::PointerButton::Primary) && !gizmo_clicked {
            self.selection_box_start = response.interact_pointer_pos();
            self.selection_box_end = self.selection_box_start;
        }
        if self.selection_box_start.is_some() && response.dragged_by(egui::PointerButton::Primary) {
            self.selection_box_end = response.interact_pointer_pos();
        }
        if self.selection_box_start.is_some() && response.drag_stopped_by(egui::PointerButton::Primary) {
            if let (Some(start), Some(end)) = (self.selection_box_start, self.selection_box_end) {
                let bounds = egui::Rect::from_two_pos(start, end);
                let additive = ctx.input(|i| i.modifiers.shift);
                match self.selection_mode {
                    SelectionMode::Vertex => {
                        let selected: Vec<_> = projected.iter().enumerate()
                            .filter_map(|(i, p)| p.filter(|(pos, _)| bounds.contains(*pos)).map(|_| i))
                            .collect();
                        if !additive { self.selected_vertices.clear(); }
                        self.selected_vertices.extend(selected);
                        self.selected_vertices.sort_unstable();
                        self.selected_vertices.dedup();
                    }
                    SelectionMode::Triangle => {
                        let selected: Vec<_> = mesh.indices.iter().enumerate()
                            .filter_map(|(i, tri)| {
                                let points = [projected[tri[0]], projected[tri[1]], projected[tri[2]]];
                                let center = points.iter().flatten()
                                    .fold((egui::Pos2::ZERO, 0usize), |(sum, count), (p, _)| {
                                        (sum + p.to_vec2(), count + 1)
                                    });
                                (center.1 == 3).then(|| {
                                    let center = center.0 / 3.0;
                                    bounds.contains(center).then_some(i)
                                }).flatten()
                            })
                            .collect();
                        if !additive { self.selected_triangles.clear(); }
                        self.selected_triangles.extend(poly::whole_polygons(&mesh, &selected));
                        self.selected_triangles.sort_unstable();
                        self.selected_triangles.dedup();
                    }
                    SelectionMode::Edge => {
                        let mut selected = Vec::new();
                        let mut seen = HashSet::new();
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
                                let edge = (a.min(b), a.max(b));
                                if !seen.insert(edge) { continue; }
                                if let (Some((pa, _)), Some((pb, _))) = (projected[a], projected[b]) {
                                    if bounds.contains(pa) && bounds.contains(pb) {
                                        selected.push(edge);
                                    }
                                }
                            }
                        }
                        if !additive { self.selected_edges.clear(); }
                        self.selected_edges.extend(selected);
                        self.selected_edges.sort_unstable();
                        self.selected_edges.dedup();
                    }
                }
            }
            self.selection_box_start = None;
            self.selection_box_end = None;
        }
        if response.double_clicked() && self.selection_mode == SelectionMode::Edge && !gizmo_clicked {
            if let Some(pointer) = response.interact_pointer_pos() {
                let mut best = None;
                for tri in &mesh.indices {
                    for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                        if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
                        let (Some((pa, _)), Some((pb, _))) = (projected[a], projected[b]) else { continue };
                        let ab = pb - pa;
                        let t = if ab.length_sq() > 1e-6 {
                            ((pointer - pa).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
                        } else { 0.0 };
                        let distance = pointer.distance(pa + ab * t);
                        if distance <= 16.0 && best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, (a.min(b), a.max(b))));
                        }
                    }
                }
                if let Some((_, edge)) = best {
                    pending_edge_loop = Some(edge);
                }
            }
        } else if response.clicked() && !gizmo_clicked && !box_selecting {
            if let Some(pointer) = response.interact_pointer_pos() {
                let front_only = self.render_mode != RenderMode::Wireframe;
                let triangle_hit = |tri: [usize; 3]| -> Option<(f32, f32)> {
                    let (a, za) = projected[tri[0]]?;
                    let (b, zb) = projected[tri[1]]?;
                    let (c, zc) = projected[tri[2]]?;
                    let v0 = b - a;
                    let v1 = c - a;
                    let v2 = pointer - a;
                    let denom = v0.x * v1.y - v1.x * v0.y;
                    if denom.abs() < 1e-6 {
                        return None;
                    }
                    let u = (v2.x * v1.y - v1.x * v2.y) / denom;
                    let v = (v0.x * v2.y - v2.x * v0.y) / denom;
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                        Some((za + u * (zb - za) + v * (zc - za), 0.0))
                    } else {
                        None
                    }
                };
                let front_depth = || {
                    mesh.indices
                        .iter()
                        .filter_map(|&tri| triangle_hit(tri).map(|(depth, _)| depth))
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                };
                match self.selection_mode {
                    SelectionMode::Vertex => {
                        let hit = projected
                            .iter()
                            .enumerate()
                            .filter_map(|(i, p)| {
                                let (pos, depth) = *p.as_ref()?;
                                let distance = pos.distance(pointer);
                                if front_only {
                                    let Some(front) = front_depth() else {
                                        return None;
                                    };
                                    if depth > front + 0.5 {
                                        return None;
                                    }
                                }
                                (distance <= 14.0).then_some((distance, depth, i))
                            })
                            .min_by(|a, b| {
                                if front_only {
                                    a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                                } else {
                                    a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                                }
                            })
                            .map(|(_, _, i)| i);
                        if let Some(i) = hit {
                            if let Some(position) = self.selected_vertices.iter().position(|&v| v == i) {
                                self.selected_vertices.remove(position);
                            } else {
                                self.selected_vertices.push(i);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                    SelectionMode::Triangle => {
                        let mut candidates = mesh.indices.iter().enumerate().filter_map(|(i, &tri)| {
                            let (depth, _) = triangle_hit(tri)?;
                            Some((depth, i))
                        }).collect::<Vec<_>>();
                        candidates.sort_by(|a, b| {
                            if front_only {
                                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                            } else {
                                a.1.cmp(&b.1)
                            }
                        });
                        let hit = candidates.first().map(|(_, i)| *i);
                        if let Some(i) = hit {
                            // Clicking picks (or unpicks) the whole polygon.
                            let face = poly::whole_polygons(&mesh, &[i]);
                            if self.selected_triangles.contains(&i) {
                                self.selected_triangles.retain(|t| !face.contains(t));
                            } else {
                                self.selected_triangles.extend(face);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                    SelectionMode::Edge => {
                        let mut best: Option<(f32, f32, (usize, usize))> = None;
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
                                let Some((pa, za)) = projected[a] else { continue };
                                let Some((pb, zb)) = projected[b] else { continue };
                                let ab = pb - pa;
                                let denom = ab.length_sq();
                                let t = if denom > 1e-6 {
                                    ((pointer - pa).dot(ab) / denom).clamp(0.0, 1.0)
                                } else { 0.0 };
                                let distance = pointer.distance(pa + ab * t);
                                let depth = (za + zb) * 0.5;
                                if front_only {
                                    let Some(front) = front_depth() else {
                                        continue;
                                    };
                                    if depth > front + 0.5 {
                                        continue;
                                    }
                                }
                                let better = best.map_or(true, |current| {
                                    if front_only { depth < current.1 } else { distance < current.0 }
                                });
                                if distance <= 12.0 && better {
                                    best = Some((distance, depth, (a.min(b), b.max(a))));
                                }
                            }
                        }
                        if let Some((_, _, edge)) = best {
                            if let Some(position) = self.selected_edges.iter().position(|&v| v == edge) {
                                self.selected_edges.remove(position);
                            } else {
                                self.selected_edges.push(edge);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                }
            }
        }
        // GPU: the lit surfaces stay uploaded until the mesh, lights or display mode change,
        // so moving the camera skips building them (gpu_viewport.rs).
        let gpu_key = self.gpu_viewport.is_some().then(|| self.viewport_scene_key());
        let gpu_fresh = gpu_key.is_some_and(|key| self.gpu_viewport.as_ref().is_some_and(|gpu| gpu.has_scene(key)));
        let built = if gpu_fresh { &Mesh::default() } else { mesh };
        // Stored normals (Normal node) shade the surface; otherwise they're computed.
        let corner_normals = sops::corner_normals(built);
        // UV Quick Shade: the texture and each triangle corner's UV.
        let quickshade = sops::quickshade_file(built).and_then(|file| sops::corner_uvs(built).map(|uvs| (quickshade_texture(&file), uvs)));
        let textured = quickshade.is_some() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals);
        let ambient = self.ambient_light.clamp(0.0, 1.0);
        let lights = self.scene_lights();
        let environment = self.environment_light().and_then(|(file, scale, rotation, _)| envlight::load(&file).ok().map(|map| (map, scale, rotation)));
        let illumination = |normal: Vec3, point: Vec3| -> [f32; 3] {
            let mut rgb = [ambient; 3];
            // Image-based diffuse light from the HDRI.
            if let Some((map, scale, rotation)) = &environment {
                let e = map.diffuse(normal, *rotation);
                for c in 0..3 {
                    rgb[c] += e[c] * scale[c];
                }
            }
            for (position, direction, color, intensity, kind) in &lights {
                let to_light = if *kind == 1 {
                    (*position).sub(point).normalize()
                } else {
                    direction.scale(-1.0)
                };
                // Inverse-square, like the MoonRay sphere lights it renders as.
                let attenuation = if *kind == 1 {
                    (POINT_LIGHT_FALLOFF / (*position).sub(point).length().powi(2).max(1e-4)).min(1.5)
                } else {
                    1.0
                };
                let amount = *intensity * attenuation * normal.dot(to_light).max(0.0);
                for channel in 0..3 {
                    rgb[channel] += amount * color[channel];
                }
            }
            rgb.map(|value| value.clamp(0.0, 1.5))
        };

        struct Poly {
            pts: [egui::Pos2; 3],
            // Camera-space depth of each corner, for the depth buffer.
            depth: [f32; 3],
            colors: [egui::Color32; 3],
            // Texture coordinates when the mesh is quick-shaded.
            uv: Option<[egui::Pos2; 3]>,
            // Which edges (0-1, 1-2, 2-0) the wireframe draws: polygon outlines, not their diagonals.
            edges: [bool; 3],
        }
        let cd = mesh.point_attributes.get("Cd");
        let cd_color = |index: usize| -> Option<egui::Color32> {
            let values = cd?;
            let vertex_count = mesh.vertices.len();
            let (r, g, b) = if values.len() >= vertex_count.saturating_mul(3) {
                (
                    values.get(index * 3).copied()?,
                    values.get(index * 3 + 1).copied()?,
                    values.get(index * 3 + 2).copied()?,
                )
            } else {
                let value = values.get(index).copied()?;
                let [r, g, b] = attribute_color(value);
                (r, g, b)
            };
            Some(egui::Color32::from_rgb(
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
            ))
        };
        // Triangle corner colours follow Houdini's precedence: the first Cd found
        // on vertices, points, primitives, then the detail.
        let (point_count, prim_count) = (mesh.vertices.len(), mesh.indices.len());
        let surface_cd = [
            ("vertex:Cd", SheetDomain::Vertices, prim_count * 3),
            ("Cd", SheetDomain::Points, point_count),
            ("prim:Cd", SheetDomain::Primitives, prim_count),
            ("detail:Cd", SheetDomain::Detail, 1),
        ]
        .into_iter()
        .find_map(|(key, domain, count)| {
            let values = mesh.point_attributes.get(key)?;
            (count > 0 && values.len() >= count).then(|| (values.as_slice(), values.len() / count, domain))
        });
        let corner_color = |tri_index: usize, corner: usize, point: usize| -> Option<egui::Color32> {
            let (values, components, domain) = surface_cd?;
            let index = match domain {
                SheetDomain::Vertices => tri_index * 3 + corner,
                SheetDomain::Points => point,
                SheetDomain::Primitives => tri_index,
                SheetDomain::Detail => 0,
            };
            let [r, g, b] = if components >= 3 {
                let at = index * components;
                [*values.get(at)?, *values.get(at + 1)?, *values.get(at + 2)?]
            } else {
                attribute_color(*values.get(index * components)?)
            };
            Some(egui::Color32::from_rgb(
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
            ))
        };
        let wire: Option<[u8; 4]> = match self.render_mode {
            RenderMode::Wireframe => Some([80, 220, 120, 255]),
            RenderMode::ShadedWireframe => Some([0, 0, 0, 110]),
            _ => None,
        };
        let mut queue: Vec<Poly> = Vec::new();
        let (mut gpu_faces, mut gpu_edges) = (Vec::new(), Vec::new());
        let outlines = poly::outline_edges(built);
        // Toon Shader faces: cel-shaded per pixel on the GPU, with outlines (gpu_viewport.rs).
        let toon_ids = built.point_attributes.get(TOON_ATTR).filter(|v| v.len() == built.indices.len() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals));
        let mut toon_slots: HashMap<usize, usize> = HashMap::new();
        let mut toon_table: Vec<gpu_viewport::ToonMaterial> = Vec::new();
        let mut gpu_toon: Vec<gpu_viewport::ToonVertex> = Vec::new();
        // Smooth point normals push the outline out without gaps at hard edges.
        let smooth_normals = if toon_ids.is_some() { built.compute_normals() } else { Vec::new() };
        // See-through faces: the material's opacity, times Alpha when it uses point alpha.
        let opacity = built.point_attributes.get(OPACITY_ATTR).filter(|v| v.len() == built.indices.len() * 2);
        let point_alpha = built.point_attributes.get("Alpha").filter(|a| a.len() == built.vertices.len());
        let prim_alpha = built.point_attributes.get("prim:Alpha").filter(|a| a.len() == built.indices.len());
        let corner_alpha = |tri_index: usize, point: usize| -> f32 {
            let Some(o) = opacity else { return 1.0 };
            let (amount, uses_alpha) = (o[tri_index * 2], o[tri_index * 2 + 1] > 0.5);
            let alpha = if uses_alpha { point_alpha.map(|a| a[point]).or(prim_alpha.map(|a| a[tri_index])).unwrap_or(1.0) } else { 1.0 };
            (amount * alpha).clamp(0.0, 1.0)
        };
        for (tri_index, tri) in built.indices.iter().enumerate() {
            if self.isolate_selection {
                let keep = match self.selection_mode {
                    SelectionMode::Vertex => self.selected_vertices.iter().any(|&i| tri.contains(&i)),
                    SelectionMode::Triangle => self.selected_triangles.contains(&tri_index),
                    SelectionMode::Edge => self.selected_edges.iter().any(|&(a, b)| {
                        tri.contains(&a) && tri.contains(&b)
                    }),
                };
                if !keep {
                    continue;
                }
            }
            let (v0, v1, v2) = (mesh.vertices[tri[0]], mesh.vertices[tri[1]], mesh.vertices[tri[2]]);
            let face_normal = v1.sub(v0).cross(v2.sub(v0)).normalize();
            let shade = |n: Vec3, point: Vec3| -> egui::Color32 {
                match self.render_mode {
                    RenderMode::Wireframe => egui::Color32::TRANSPARENT,
                    RenderMode::Normals => egui::Color32::from_rgb(
                        ((n.x * 0.5 + 0.5) * 255.0) as u8,
                        ((n.y * 0.5 + 0.5) * 255.0) as u8,
                        ((n.z * 0.5 + 0.5) * 255.0) as u8,
                    ),
                    RenderMode::FlatShaded => {
                        let rgb = illumination(face_normal, point);
                        egui::Color32::from_rgb(
                            (200.0 * rgb[0]) as u8,
                            (205.0 * rgb[1]) as u8,
                            (215.0 * rgb[2]) as u8,
                        )
                    }
                    _ => {
                        let rgb = illumination(n, point);
                        egui::Color32::from_rgb(
                            (200.0 * rgb[0]) as u8,
                            (205.0 * rgb[1]) as u8,
                            (215.0 * rgb[2]) as u8,
                        )
                    }
                }
            };
            let lit_color = |corner: usize, point: Vec3, normal: Vec3| {
                let Some(base) = corner_color(tri_index, corner, tri[corner]) else {
                    return shade(normal, point);
                };
                let lighting_normal = if self.render_mode == RenderMode::FlatShaded {
                    face_normal
                } else {
                    normal
                };
                let light = illumination(lighting_normal, point);
                egui::Color32::from_rgb(
                    (f32::from(base.r()) * light[0]).clamp(0.0, 255.0) as u8,
                    (f32::from(base.g()) * light[1]).clamp(0.0, 255.0) as u8,
                    (f32::from(base.b()) * light[2]).clamp(0.0, 255.0) as u8,
                )
            };
            let corner_n = corner_normals[tri_index];
            let colors = if textured {
                // White, lit; the texture supplies the colour.
                let lit = |point: Vec3, normal: Vec3| {
                    let n = if self.render_mode == RenderMode::FlatShaded { face_normal } else { normal };
                    let l = illumination(n, point);
                    egui::Color32::from_rgb((255.0 * l[0]).min(255.0) as u8, (255.0 * l[1]).min(255.0) as u8, (255.0 * l[2]).min(255.0) as u8)
                };
                [lit(v0, corner_n[0]), lit(v1, corner_n[1]), lit(v2, corner_n[2])]
            } else {
                [lit_color(0, v0, corner_n[0]), lit_color(1, v1, corner_n[1]), lit_color(2, v2, corner_n[2])]
            };
            let uv = quickshade.as_ref().filter(|_| textured).map(|(_, uvs)| uvs[tri_index].map(|c| egui::pos2(c[0], 1.0 - c[1])));
            // A toon face: its material's slot in the table, its corners' colours and light.
            let toon = toon_ids.map(|ids| ids[tri_index]).filter(|id| *id >= 0.5).and_then(|id| {
                let id = (id - 1.0).round() as usize;
                let look = match &self.graph.nodes.get(&id)?.kind {
                    NodeKind::Shader(v) if v.table == shading::Table::Toon => v.toon_look(),
                    _ => return None,
                };
                let slot = *toon_slots.entry(id).or_insert_with(|| {
                    toon_table.push(look);
                    toon_table.len() - 1
                });
                let corners = [v0, v1, v2];
                let base = |k: usize| corner_color(tri_index, k, tri[k]).map_or([0.8; 3], |c| [c.r(), c.g(), c.b()].map(|v| v as f32 / 255.0));
                let light = |k: usize| {
                    let n = if self.render_mode == RenderMode::FlatShaded { face_normal } else { corner_n[k] };
                    let rgb = illumination(n, corners[k]);
                    ((rgb[0] + rgb[1] + rgb[2]) / 3.0 - ambient).clamp(0.0, 1.0)
                };
                Some((slot.min(gpu_viewport::MAX_TOONS - 1), look, [base(0), base(1), base(2)], [light(0), light(1), light(2)]))
            });
            let colors = match &toon {
                // The CPU viewport bands each corner.
                Some((_, look, base, light)) if gpu_key.is_none() => std::array::from_fn(|k| {
                    let c = look.shade(base[k], light[k]);
                    egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
                }),
                _ => colors,
            };
            if gpu_key.is_some() {
                // World-space corners; the GPU does the camera.
                let corner = |k: usize| [[v0, v1, v2][k].x, [v0, v1, v2][k].y, [v0, v1, v2][k].z];
                let rgba = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
                if let Some((slot, _, base, light)) = &toon {
                    gpu_toon.extend((0..3).map(|k| {
                        let n = smooth_normals.get(tri[k]).copied().unwrap_or(face_normal);
                        gpu_viewport::ToonVertex { position: corner(k), light: light[k], color: [base[k][0], base[k][1], base[k][2], 1.0], normal: [n.x, n.y, n.z], material: *slot as f32 }
                    }));
                } else if self.render_mode != RenderMode::Wireframe {
                    // Wireframe mode shows every edge, so its (transparent) faces don't occlude.
                    gpu_faces.extend((0..3).map(|k| {
                        let mut colour = rgba(colors[k]);
                        colour[3] = corner_alpha(tri_index, tri[k]);
                        gpu_viewport::Vertex::new(corner(k), colour, uv.map(|uv| [uv[k].x, uv[k].y]))
                    }));
                }
                // Toon faces have their outline instead of the wireframe.
                if let Some(c) = wire.filter(|_| toon.is_none()) {
                    let colour = c.map(|v| v as f32 / 255.0);
                    gpu_edges.extend((0..3).filter(|&k| outlines[tri_index][k]).flat_map(|k| [k, (k + 1) % 3]).map(|k| gpu_viewport::Vertex::new(corner(k), colour, None)));
                }
                continue;
            }
            let camera_triangle = [to_cam(v0), to_cam(v1), to_cam(v2)];
            let clipped = clip_triangle(camera_triangle);
            if clipped.len() < 3 {
                continue;
            }
            let first = clipped[0];
            for i in 1..clipped.len() - 1 {
                let a = project_cam(first);
                let b = project_cam(clipped[i]);
                let c = project_cam(clipped[i + 1]);
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if !area.is_finite() || area.abs() < 1e-4 {
                    continue;
                }
                queue.push(Poly {
                    // Near-plane clipped pieces fall back to untextured shading (and show all their edges).
                    uv: if clipped.len() == 3 { uv } else { None },
                    edges: if clipped.len() == 3 { outlines[tri_index] } else { [true; 3] },
                    pts: [a, b, c],
                    depth: [first.z, clipped[i].z, clipped[i + 1].z],
                    colors: if clipped.len() == 3 {
                        colors
                    } else {
                        let average = egui::Color32::from_rgb(
                            ((colors[0].r() as u16 + colors[1].r() as u16 + colors[2].r() as u16) / 3) as u8,
                            ((colors[0].g() as u16 + colors[1].g() as u16 + colors[2].g() as u16) / 3) as u8,
                            ((colors[0].b() as u16 + colors[1].b() as u16 + colors[2].b() as u16) / 3) as u8,
                        );
                        [average; 3]
                    },
                });
            }
        }
        // Surfaces and wireframe go through a depth buffer (gpu_viewport.rs, else
        // raster.rs), at the screen's pixel density, then onto the viewport as one image.
        let view_display = self.view_displays().first().copied();
        // Geometry containers display their inner display node's kind.
        let view_display_node = view_display.and_then(|id| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Geometry { .. }) => self.graph.display_of(id),
            _ => Some(id),
        });
        let particle_display = view_display_node
            .and_then(|id| self.graph.nodes.get(&id))
            .is_some_and(|node| matches!(
                node.kind,
                NodeKind::Add { .. }
                    | NodeKind::Particles { .. }
                        | NodeKind::Flip { .. }
                    | NodeKind::WhitewaterSolver(_)
                    | NodeKind::Cache { .. }
            ));
        let point_color = view_display_node
            .and_then(|id| self.graph.nodes.get(&id))
            .map(|node| match node.kind {
                NodeKind::Flip { .. } => egui::Color32::from_rgb(70, 180, 255),
                _ => egui::Color32::from_rgb(255, 170, 45),
            })
            .unwrap_or(egui::Color32::from_rgb(255, 220, 80));
        let point_attribute_color = |index: usize| -> egui::Color32 {
            cd_color(index).unwrap_or(point_color)
        };
        // Hair and other curves (see groom.rs) are drawn as lines, not loose points.
        let curve_ranges = groom::curves(mesh);
        let referenced = used_points(mesh, &curve_ranges);
        let has_unreferenced_points = !kinefx::Skeleton::is_skeleton(mesh) && referenced.iter().any(|&is_referenced| !is_referenced);
        let point_count = mesh.vertices.len();
        // Loose points with Alpha (Pyro, smoke, particles) are soft splats. A surface's
        // Alpha is its opacity (Principled "Use point alpha"), so its points aren't.
        let splat_alpha = mesh.point_attributes.get("Alpha").filter(|a| a.len() == point_count && point_count > 0 && has_unreferenced_points);
        if gpu_key.is_some() || !queue.is_empty() {
            // The GPU draws at full pixel density; the CPU is capped to stay interactive.
            let max_pixels: f32 = if self.gpu_viewport.is_some() { 1.6e7 } else { 2.5e6 };
            let size = response.rect.size();
            let mut scale = ctx.pixels_per_point();
            if size.x * size.y * scale * scale > max_pixels {
                scale = (max_pixels / (size.x * size.y)).sqrt();
            }
            let (w, h) = ((size.x * scale).round().max(1.0) as usize, (size.y * scale).round().max(1.0) as usize);
            let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            // The points stay uploaded too, until the mesh or what decides their look changes.
            let points_key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (self.mesh_revision, self.show_points, particle_display, point_color.to_array()).hash(&mut h);
                // Smoke is lit by the scene's lights, so moving one redraws it.
                format!("{:?}", self.scene_lights()).hash(&mut h);
                h.finish()
            };
            let volume_lights = self.volume_lights();
            // A flipbook or real-time render frame waiting to be rendered, once this frame's
            // scene is uploaded: through the render camera at its size, or the viewport's view.
            let capture = self.flipbook_recording.as_ref().filter(|r| r.waiting).and_then(|r| r.capture.as_ref()).map(|c| c.render);
            let render_camera = capture.flatten().map(|size| (self.resolve_render_camera().0.realtime_camera(size, near_plane, focal), size));
            if let (Some(gpu), Some(key)) = (self.gpu_viewport.as_mut(), gpu_key) {
                if !gpu_fresh {
                    gpu.set_scene(key, &gpu_faces, &gpu_edges, &gpu_toon, &toon_table, quickshade.as_ref().map(|(texture, _)| texture.clone()));
                }
                if !gpu.has_points(points_key) {
                    let at = |i: usize| [mesh.vertices[i].x, mesh.vertices[i].y, mesh.vertices[i].z];
                    let (mut dots, mut splats) = (Vec::new(), Vec::new());
                    // Hair and other curves, piece by piece, as thick as their width attribute
                    // (a plain line where they have none), in their Cd or hair colour.
                    let widths = mesh.point_attributes.get("width").filter(|w| w.len() == point_count);
                    let width = |i: usize| widths.map_or(-1.0, |w| w[i].max(0.0));
                    let curves: Vec<gpu_viewport::Strand> = curve_ranges
                        .iter()
                        .flat_map(|range| {
                            let c = cd_color(range.start).unwrap_or(egui::Color32::from_rgb(200, 165, 115));
                            let color = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
                            (range.start..range.end.saturating_sub(1)).map(move |i| gpu_viewport::Strand { a: at(i), width_a: width(i), b: at(i + 1), width_b: width(i + 1), color })
                        })
                        .collect();
                    if let Some(alpha) = splat_alpha {
                        // Soft splats in world units, as opaque at the centre as the CPU's two discs.
                        let pscale = mesh.point_attributes.get("pscale").filter(|v| v.len() == point_count);
                        // Smoke lit by the scene's lights, when it has any.
                        let relit = (!volume_lights.is_empty()).then(|| sims::relight_smoke(mesh, &volume_lights)).flatten();
                        let colors = relit.as_ref().or_else(|| mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3));
                        let emission = mesh.point_attributes.get("emission").filter(|v| v.len() == point_count * 3);
                        let triple = |values: Option<&Vec<f32>>, i: usize, default: f32| values.map_or([default; 3], |v| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]]);
                        splats.extend((0..point_count).filter(|&i| alpha[i] > 0.004 && !referenced[i]).map(|i| {
                            let a = alpha[i].clamp(0.0, 1.0);
                            let [r, g, b] = triple(colors, i, 0.8).map(|c| c.clamp(0.0, 1.0));
                            gpu_viewport::Point::new(at(i), pscale.map_or(0.05, |p| p[i]), [r, g, b, 1.0 - (1.0 - a * 0.5) * (1.0 - a * 0.6)], triple(emission, i, 0.0))
                        }));
                    } else if self.show_points || particle_display || has_unreferenced_points {
                        let radius = if particle_display || has_unreferenced_points { 3.5 } else { 2.0 };
                        dots.extend((0..point_count).filter(|&i| !(has_unreferenced_points && referenced[i])).map(|i| {
                            let c = point_attribute_color(i);
                            gpu_viewport::Point::new(at(i), radius, [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0], [0.0; 3])
                        }));
                    }
                    gpu.set_points(points_key, &dots, &splats, &curves);
                }
                let camera = gpu_viewport::Camera {
                    target: [target.x, target.y, target.z],
                    rotation: [sin_x, cos_x, sin_y, cos_y],
                    zoom,
                    center: center - rect_min,
                    size,
                    focal,
                    ortho_scale,
                    orthographic: self.orthographic,
                    near: near_plane,
                };
                let id = gpu.draw([w as u32, h as u32], &camera);
                painter.image(id, response.rect, full, egui::Color32::WHITE);
                if capture.is_some() {
                    let frame = match render_camera {
                        // Transparent where empty (premultiplied, as egui images are).
                        Some((render, size)) => {
                            let wants_glow = self.flipbook_recording.as_ref().and_then(|r| r.capture.as_ref()).and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.glow);
                            let (pixels, glow) = if wants_glow { gpu.render_final_glow(size, &render) } else { (gpu.render_final(size, &render), None) };
                            if wants_glow {
                                let glow = spritesheet::encode_glow([size[0] as usize, size[1] as usize], glow.as_deref());
                                if let Some(recording) = self.flipbook_recording.as_mut() {
                                    recording.glows.push(glow);
                                }
                            }
                            egui::ColorImage::new([size[0] as usize, size[1] as usize], pixels.into_iter().map(|[r, g, b, a]| egui::Color32::from_rgba_premultiplied(r, g, b, a)).collect())
                        }
                        // The viewport's view, at most flipbook::MAX_WIDTH wide, over its background.
                        None => {
                            let width = (w as u32).min(flipbook::MAX_WIDTH);
                            let size = [width, ((h as f32 * width as f32 / w as f32).round() as u32).max(1)];
                            let bg = self.viewport_bg as u32;
                            let over = |c: u8, a: u8| (c as u32 + bg * (255 - a as u32) / 255).min(255) as u8;
                            let pixels = gpu.render(size, &camera);
                            egui::ColorImage::new([size[0] as usize, size[1] as usize], pixels.into_iter().map(|[r, g, b, a]| egui::Color32::from_rgb(over(r, a), over(g, a), over(b, a))).collect())
                        }
                    };
                    if let Some(recording) = self.flipbook_recording.as_mut() {
                        recording.frames.push(frame);
                        recording.next += 1;
                        recording.waiting = false;
                    }
                }
            } else {
                let mut raster = raster::Raster::new(w, h, self.orthographic);
                let to_pixel = |p: egui::Pos2, z: f32| raster::ScreenVertex { x: (p.x - rect_min.x) * scale, y: (p.y - rect_min.y) * scale, z };
                let rgb = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
                let texture = quickshade.as_ref().map(|(texture, _)| texture.clone());
                // Wireframe mode shows every edge, so its (transparent) faces don't occlude.
                let triangles: Vec<raster::Triangle> = if self.render_mode == RenderMode::Wireframe {
                    Vec::new()
                } else {
                    queue
                        .iter()
                        .map(|p| raster::Triangle {
                            v: [0, 1, 2].map(|k| to_pixel(p.pts[k], p.depth[k])),
                            rgb: p.colors.map(rgb),
                            uv: p.uv.zip(texture.as_deref()).map(|(uv, t)| (uv.map(|c| [c.x, c.y]), t)),
                        })
                        .collect()
                };
                let lines: Vec<raster::Line> = match wire {
                    Some(colour) => queue
                        .iter()
                        .flat_map(|p| [(0, 1), (1, 2), (2, 0)].into_iter().filter(|(a, _)| p.edges[*a]).map(|(a, b)| raster::Line { a: to_pixel(p.pts[a], p.depth[a]), b: to_pixel(p.pts[b], p.depth[b]), rgba: colour }))
                        .collect(),
                    None => Vec::new(),
                };
                raster.draw(&triangles, &lines);
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], raster.color.as_flattened());
                let texture = viewport_texture(ctx, image);
                painter.image(texture.id(), response.rect, full, egui::Color32::WHITE);
            }
        }

        if !curve_ranges.is_empty() && self.gpu_viewport.is_none() {
            // Dense grooms are thinned for display; every curve still renders.
            const MAX_DRAWN_CURVES: usize = 8_000;
            let stride = curve_ranges.len().div_ceil(MAX_DRAWN_CURVES).max(1);
            let hair = egui::Color32::from_rgb(200, 165, 115);
            for range in curve_ranges.iter().step_by(stride) {
                let line: Vec<egui::Pos2> = range.clone().filter_map(|i| projected[i].map(|p| p.0)).collect();
                if line.len() >= 2 {
                    let colour = cd_color(range.start).unwrap_or(hair);
                    painter.add(egui::Shape::line(line, egui::Stroke::new(1.0, colour)));
                }
            }
        }
        // KineFX skeletons: a bone from each joint to its parent, and a dot per joint.
        if kinefx::Skeleton::is_skeleton(mesh) {
            let parent = &mesh.point_attributes["parent"];
            let bone = egui::Color32::from_rgb(90, 220, 255);
            for (j, p) in parent.iter().enumerate() {
                if *p >= 0.0 {
                    if let (Some(a), Some(b)) = (projected.get(j).copied().flatten(), projected.get(*p as usize).copied().flatten()) {
                        painter.line_segment([a.0, b.0], egui::Stroke::new(2.5, bone));
                    }
                }
                if let Some(a) = projected.get(j).copied().flatten() {
                    painter.circle_filled(a.0, 3.5, egui::Color32::from_rgb(255, 230, 120));
                }
            }
        }
        // Volume points (Pyro output) carry Alpha and pscale: draw them as soft,
        // back-to-front splats sized in world units instead of opaque dots. (The GPU
        // draws these and the points below itself, depth-tested and uncapped.)
        if let Some(alpha) = splat_alpha.filter(|_| self.gpu_viewport.is_none()) {
            let pscale = mesh.point_attributes.get("pscale").filter(|v| v.len() == point_count);
            let colors = mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3);
            // Thin out very dense volumes, growing the remaining splats to compensate.
            const MAX_SPLATS: usize = 40_000;
            let stride = point_count.div_ceil(MAX_SPLATS).max(1);
            let grow = (stride as f32).cbrt();
            let mut order: Vec<(usize, f32)> = projected
                .iter()
                .enumerate()
                .step_by(stride)
                .filter_map(|(i, p)| p.map(|(_, depth)| (i, depth)))
                .filter(|(i, _)| alpha[*i] > 0.004 && !referenced[*i])
                .collect();
            order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (i, depth) in order {
                let Some((position, _)) = projected[i] else { continue };
                let world_radius = pscale.map_or(0.05, |p| p[i]) * grow;
                let radius = if self.orthographic { world_radius * ortho_scale } else { world_radius / depth.max(1e-3) * focal };
                let [r, g, b] = colors.map_or([0.8; 3], |c| [c[i * 3], c[i * 3 + 1], c[i * 3 + 2]]).map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8);
                let a = 1.0 - (1.0 - alpha[i].clamp(0.0, 1.0)).powf(grow);
                let shade = |opacity: f32| egui::Color32::from_rgba_unmultiplied(r, g, b, (opacity.clamp(0.0, 1.0) * 255.0) as u8);
                // Two nested discs approximate a soft falloff.
                painter.circle_filled(position, radius.clamp(0.5, 120.0), shade(a * 0.5));
                painter.circle_filled(position, (radius * 0.55).clamp(0.5, 120.0), shade(a * 0.6));
            }
        }
        if (self.show_points || particle_display || has_unreferenced_points)
            && self.gpu_viewport.is_none()
            && projected.len() <= 50_000
            && splat_alpha.is_none()
        {
            let point_radius = if particle_display || has_unreferenced_points { 3.5 } else { 2.0 };
            for (index, point) in projected.iter().enumerate() {
                if has_unreferenced_points && referenced[index] {
                    continue;
                }
                let Some(p) = point else { continue };
                painter.circle_filled(p.0, point_radius, point_attribute_color(index));
            }
        }
        for &i in self.selected_vertices.iter().filter(|&&i| i < projected.len()) {
            if let Some((position, _)) = projected[i] {
                painter.circle_stroke(
                    position,
                    7.0,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
                painter.circle_filled(position, 3.0, egui::Color32::YELLOW);
            }
        }
        for &i in self.selected_triangles.iter().filter(|&&i| i < mesh.indices.len()) {
            let tri = mesh.indices[i];
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                if let (Some(pa), Some(pb)) = (projected[a], projected[b]) {
                    painter.line_segment(
                        [pa.0, pb.0],
                        egui::Stroke::new(3.0, egui::Color32::YELLOW),
                    );
                }
            }
        }
        for &(a, b) in &self.selected_edges {
            if let (Some(pa), Some(pb)) = (projected.get(a).copied().flatten(), projected.get(b).copied().flatten()) {
                painter.line_segment(
                    [pa.0, pb.0],
                    egui::Stroke::new(4.0, egui::Color32::from_rgb(255, 150, 40)),
                );
            }
        }
        if let (Some(start), Some(end)) = (self.selection_box_start, self.selection_box_end) {
            let selection_rect = egui::Rect::from_two_pos(start, end);
            painter.rect_filled(
                selection_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(70, 150, 240, 35),
            );
            painter.rect_stroke(
                selection_rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 190, 255)),
                egui::StrokeKind::Inside,
            );
        }

        if self.show_gizmos {
            let mut ids: Vec<NodeId> = self
                .graph
                .nodes
                .iter()
                .filter(|(_, n)| {
                    n.parent.is_none() && matches!(n.kind, NodeKind::Camera { .. } | NodeKind::Light { .. })
                })
                .map(|(id, _)| *id)
                .collect();
            ids.sort_unstable();
            // Gizmos keep a roughly constant on-screen size as the view zooms.
            let unit = zoom * 0.07;
            for id in ids {
                let node = &self.graph.nodes[&id];
                let selected = self.selected == Some(id);
                match &node.kind {
                    NodeKind::Light { position, rotation, color, kind, .. } => {
                        let pos = Vec3::from_arr(*position);
                        let channel = |v: f32| (90.0 + v.clamp(0.0, 1.0) * 165.0) as u8;
                        let tint = if node.bypass {
                            egui::Color32::from_gray(110)
                        } else {
                            egui::Color32::from_rgb(channel(color[0]), channel(color[1]), channel(color[2]))
                        };
                        let outline = if selected { egui::Color32::WHITE } else { tint };
                        let stroke = egui::Stroke::new(if selected { 2.0 } else { 1.5 }, outline);

                        if *kind != 1 {
                            let dir = Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize();
                            let tip = pos.add(dir.scale(unit * 3.5));
                            line3d(pos, tip, egui::Stroke::new(1.0, tint));
                            let hint = if dir.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                            let side = dir.cross(hint).normalize().scale(unit * 0.25);
                            let lift = dir.cross(side).normalize().scale(unit * 0.25);
                            let base = tip.sub(dir.scale(unit * 0.6));
                            for offset in [side, side.scale(-1.0), lift, lift.scale(-1.0)] {
                                line3d(tip, base.add(offset), egui::Stroke::new(1.0, tint));
                            }
                        }

                        let cam_pos = to_cam(pos);
                        if cam_pos.z > near_plane {
                            let p = project_cam(cam_pos);
                            if response.rect.contains(p) {
                                let at = |x: f32, y: f32| p + egui::vec2(x, y);
                                let glass = at(0.0, -6.0);
                                painter.circle_filled(
                                    glass,
                                    8.0,
                                    egui::Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), if node.bypass { 20 } else { 70 }),
                                );
                                painter.circle_stroke(glass, 8.0, stroke);
                                painter.line_segment([at(-4.5, 0.5), at(-3.5, 5.0)], stroke);
                                painter.line_segment([at(4.5, 0.5), at(3.5, 5.0)], stroke);
                                for (y0, y1, half) in [(5.0, 8.0, 3.5), (9.0, 12.0, 2.5)] {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(at(-half, y0), at(half, y1)),
                                        1.0,
                                        egui::Color32::from_gray(150),
                                    );
                                }
                                let filament = [at(-2.0, 0.0), at(-2.0, -5.0), at(0.0, -3.0), at(2.0, -5.0), at(2.0, 0.0)];
                                painter.add(egui::Shape::line(filament.to_vec(), egui::Stroke::new(1.0, tint)));
                                if !node.bypass {
                                    for step in 0..8 {
                                        let angle = std::f32::consts::TAU * step as f32 / 8.0;
                                        let ray = egui::vec2(angle.cos(), angle.sin());
                                        if ray.y > 0.6 {
                                            continue;
                                        }
                                        painter.line_segment([glass + ray * 12.0, glass + ray * 16.0], egui::Stroke::new(1.0, tint));
                                    }
                                }
                                painter.text(
                                    at(0.0, 20.0),
                                    egui::Align2::CENTER_TOP,
                                    format!("light{id}"),
                                    egui::FontId::proportional(11.0),
                                    outline,
                                );
                            }
                        }
                    }
                    NodeKind::Camera { position, rotation, pan, zoom: cam_zoom, .. } => {
                        let (sx, cx) = rotation[0].to_radians().sin_cos();
                        let (sy, cy) = rotation[1].to_radians().sin_cos();
                        let right = Vec3::new(cy, 0.0, -sy);
                        let up = Vec3::new(-sx * sy, cx, -sx * cy);
                        let fwd = Vec3::new(cx * sy, sx, cx * cy);
                        let pan_scale = *cam_zoom / focal.max(1.0);
                        let aim = Vec3::from_arr(*position)
                            .sub(right.scale(pan[0] * pan_scale))
                            .add(up.scale(pan[1] * pan_scale));
                        let eye = aim.sub(fwd.scale(*cam_zoom));
                        let tint = if node.bypass {
                            egui::Color32::from_gray(110)
                        } else if self.active_camera == Some(id) {
                            egui::Color32::from_rgb(255, 170, 60)
                        } else {
                            egui::Color32::from_rgb(80, 180, 220)
                        };
                        let stroke = egui::Stroke::new(if selected { 2.5 } else { 1.5 }, if selected { egui::Color32::WHITE } else { tint });
                        let s = unit;
                        let put = |x: f32, y: f32, z: f32| {
                            eye.add(right.scale(x * s)).add(up.scale(y * s)).add(fwd.scale(z * s))
                        };

                        // Body box behind the lens point
                        let (bx, by, back) = (0.5, 0.4, -1.3);
                        let corners = |z: f32| [put(-bx, -by, z), put(bx, -by, z), put(bx, by, z), put(-bx, by, z)];
                        let (front, rear) = (corners(0.0), corners(back));
                        for i in 0..4 {
                            line3d(front[i], front[(i + 1) % 4], stroke);
                            line3d(rear[i], rear[(i + 1) % 4], stroke);
                            line3d(front[i], rear[i], stroke);
                        }

                        // Lens flaring out toward the view direction
                        let lens_len = 0.7;
                        let lens_near = [put(-0.3, -0.3, 0.0), put(0.3, -0.3, 0.0), put(0.3, 0.3, 0.0), put(-0.3, 0.3, 0.0)];
                        let lens_far = [
                            put(-0.45, -0.45, lens_len),
                            put(0.45, -0.45, lens_len),
                            put(0.45, 0.45, lens_len),
                            put(-0.45, 0.45, lens_len),
                        ];
                        for i in 0..4 {
                            line3d(lens_far[i], lens_far[(i + 1) % 4], stroke);
                            line3d(lens_near[i], lens_far[i], stroke);
                        }

                        // Two film reels on top
                        let reel_radius = 0.32;
                        for reel_z in [-0.4, -1.0] {
                            for reel_x in [-0.12, 0.12] {
                                let segments = 14;
                                let ring = |k: usize| {
                                    let a = std::f32::consts::TAU * k as f32 / segments as f32;
                                    put(reel_x, by + reel_radius + reel_radius * a.sin(), reel_z + reel_radius * a.cos())
                                };
                                for k in 0..segments {
                                    line3d(ring(k), ring(k + 1), stroke);
                                }
                            }
                        }

                        let cam_pos = to_cam(put(0.0, 0.0, back));
                        if cam_pos.z > near_plane {
                            let p = project_cam(cam_pos);
                            if response.rect.contains(p) {
                                painter.text(
                                    p + egui::vec2(0.0, 26.0),
                                    egui::Align2::CENTER_TOP,
                                    node.title(id),
                                    egui::FontId::proportional(11.0),
                                    if selected { egui::Color32::WHITE } else { tint },
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // HUD
        let bounds = mesh.bounds().map(|(lo, hi)| {
            format!(
                "bounds [{:.2}, {:.2}, {:.2}]–[{:.2}, {:.2}, {:.2}]",
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
            )
        }).unwrap_or_else(|| "bounds empty".to_string());
        let warnings = mesh.validation_warnings();
        let disp = match view_display {
            None => "No display node — enter a geometry node and double-click a node".to_string(),
            Some(id) => format!(
                "{}  |  {} pts  {} prims  |  cook {:.1} ms  |  t = {:.2}  |  cache {}/{} \n{}{}",
                self.graph.nodes.get(&id).map_or_else(|| format!("?{id}"), |n| n.title(id)),
                mesh.vertices.len(),
                poly::count(&mesh),
                self.cook_ms,
                self.time,
                self.frame_cache_hits,
                self.frame_cache.len(),
                bounds,
                if warnings.is_empty() {
                    String::new()
                } else {
                    format!("  ⚠ {}", warnings.join(", "))
                }
            ),
        };
        if !recording {
            painter.text(
                response.rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                disp,
                egui::FontId::monospace(12.0),
                egui::Color32::from_gray(210),
            );
        }
        // The gizmo for the selected node, drawn at its pivot with a constant screen size:
        // move/rotate/scale handles, or a domain box with a handle on each face.
        let mut gizmo_frame = None;
        if let Some(target) = self.gizmo_target().filter(|_| self.show_gizmos && !recording) {
            let xform = target.value;
            let pivot = target.point(Vec3::from_arr(xform.t));
            let cam_right = Vec3::new(cos_y, 0.0, -sin_y);
            let cam_up = Vec3::new(-sin_x * sin_y, cos_x, -sin_x * cos_y);
            let pivot_cam = to_cam(pivot);
            if pivot_cam.z > near_plane {
                let pivot_screen = project_cam(pivot_cam);
                let pixels_per_unit = (project_cam(to_cam(pivot.add(cam_right))) - pivot_screen).length().max(1e-4);
                let handle_px = 85.0;
                let length = handle_px / pixels_per_unit;
                let screen = |p: Vec3| {
                    let c = to_cam(p);
                    (c.z > near_plane).then(|| project_cam(c))
                };
                // Local point -> screen, and the local length that's `world` long along `dir`.
                let local_screen = |p: Vec3| screen(target.point(p));
                let local_len = |dir: Vec3, world: f32| world / target.dir(dir).length().max(1e-6);
                let centre = Vec3::from_arr(xform.t);
                let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)];
                let colours = [egui::Color32::from_rgb(235, 85, 85), egui::Color32::from_rgb(105, 215, 105), egui::Color32::from_rgb(95, 145, 245)];
                let mut parts = Vec::new();
                match (target.style, self.gizmo_mode) {
                    (GizmoStyle::Box, _) => {
                        let half = Vec3::from_arr(xform.s).scale(0.5);
                        // The box's edges.
                        let corner = |k: usize| centre.add(Vec3::new(
                            if k & 1 == 0 { -half.x } else { half.x },
                            if k & 2 == 0 { -half.y } else { half.y },
                            if k & 4 == 0 { -half.z } else { half.z },
                        ));
                        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
                            if let (Some(pa), Some(pb)) = (local_screen(corner(a)), local_screen(corner(b))) {
                                painter.line_segment([pa, pb], egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 200, 255)));
                            }
                        }
                        for i in 0..3 {
                            for plus in [true, false] {
                                let sign = if plus { 1.0 } else { -1.0 };
                                let face = centre.add(axes[i].scale(sign * [half.x, half.y, half.z][i]));
                                let out = local_len(axes[i], length * 0.3);
                                if let (Some(c), Some(tip)) = (local_screen(face), local_screen(face.add(axes[i].scale(sign * out)))) {
                                    parts.push(GizmoPart { handle: GizmoHandle::Face(i, plus), points: vec![c], axis_screen: (tip - c) / out, ring_sign: 1.0 });
                                }
                            }
                        }
                    }
                    (GizmoStyle::Xform, GizmoMode::Translate | GizmoMode::Scale) => {
                        for i in 0..3 {
                            // Scale works along the node's own (rotated) axes.
                            let axis = if self.gizmo_mode == GizmoMode::Scale { axes[i].rotate_deg(xform.r).normalize() } else { axes[i] };
                            let l = local_len(axis, length);
                            if let Some(tip) = local_screen(centre.add(axis.scale(l))) {
                                parts.push(GizmoPart { handle: GizmoHandle::Axis(i), points: vec![pivot_screen, tip], axis_screen: (tip - pivot_screen) / l, ring_sign: 1.0 });
                            }
                        }
                    }
                    (GizmoStyle::Xform, GizmoMode::Rotate) => {
                        for i in 0..3 {
                            let (u, v) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
                            let (lu, lv) = (local_len(u, length), local_len(v, length));
                            let points: Vec<egui::Pos2> = (0..=48)
                                .filter_map(|k| {
                                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                                    local_screen(centre.add(u.scale(a.cos() * lu)).add(v.scale(a.sin() * lv)))
                                })
                                .collect();
                            let (us, vs) = (local_screen(centre.add(u.scale(lu))), local_screen(centre.add(v.scale(lv))));
                            if let (true, Some(us), Some(vs)) = (points.len() > 8, us, vs) {
                                let (a, b) = (us - pivot_screen, vs - pivot_screen);
                                let sign = if a.x * b.y - a.y * b.x >= 0.0 { 1.0 } else { -1.0 };
                                parts.push(GizmoPart { handle: GizmoHandle::Ring(i), points, axis_screen: egui::Vec2::ZERO, ring_sign: sign });
                            }
                        }
                    }
                }
                if target.style == GizmoStyle::Box || self.gizmo_mode != GizmoMode::Rotate {
                    parts.push(GizmoPart { handle: GizmoHandle::Center, points: vec![pivot_screen], axis_screen: egui::Vec2::ZERO, ring_sign: 1.0 });
                }
                let frame = GizmoFrame { node: target.node, pivot: pivot_screen, pixels_per_unit, cam_right, cam_up, handle_px, parts };
                let active = self.gizmo_drag.as_ref().map(|d| d.part.handle);
                let hovered = response.hover_pos().and_then(|p| frame.hit(p)).map(|p| p.handle);
                for part in &frame.parts {
                    let highlight = active == Some(part.handle) || (active.is_none() && hovered == Some(part.handle));
                    let colour = match part.handle {
                        _ if highlight => egui::Color32::from_rgb(255, 225, 70),
                        GizmoHandle::Axis(i) | GizmoHandle::Ring(i) | GizmoHandle::Face(i, _) => colours[i],
                        GizmoHandle::Center => egui::Color32::from_gray(235),
                    };
                    match (part.handle, part.points.as_slice()) {
                        (GizmoHandle::Center, [c]) => {
                            let rect = egui::Rect::from_center_size(*c, egui::vec2(11.0, 11.0));
                            painter.rect_stroke(rect, 2.0, egui::Stroke::new(2.0, colour), egui::StrokeKind::Middle);
                        }
                        (GizmoHandle::Face(..), [c]) => {
                            // A dot with a stub pointing out of the face.
                            let out = part.axis_screen.normalized() * 14.0;
                            painter.line_segment([*c, *c + out], egui::Stroke::new(2.0, colour));
                            painter.circle_filled(*c, if highlight { 6.5 } else { 5.0 }, colour);
                        }
                        (GizmoHandle::Axis(_), [a, b]) => {
                            painter.line_segment([*a, *b], egui::Stroke::new(if highlight { 3.5 } else { 2.5 }, colour));
                            if self.gizmo_mode == GizmoMode::Scale {
                                painter.rect_filled(egui::Rect::from_center_size(*b, egui::vec2(9.0, 9.0)), 1.0, colour);
                            } else {
                                // Arrow head.
                                let dir = (*b - *a).normalized();
                                let side = egui::vec2(-dir.y, dir.x) * 5.0;
                                painter.add(egui::Shape::convex_polygon(vec![*b + dir * 10.0, *b + side, *b - side], colour, egui::Stroke::NONE));
                            }
                        }
                        (_, points) => {
                            painter.add(egui::Shape::line(points.to_vec(), egui::Stroke::new(if highlight { 3.0 } else { 2.0 }, colour)));
                        }
                    }
                }
                gizmo_frame = Some(frame);
            }
        }
        self.gizmo_frame = gizmo_frame;
        if let Some(edge) = pending_edge_loop {
            self.select_edge_loop(edge);
            self.status = "Edge loop selected".into();
        }
        if clear_selection_on_miss {
            self.clear_component_selection();
        }
        if let Some((id, painted)) = pending_paint {
            if let Some(NodeKind::AttributePaint { values, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                *values = painted;
                self.dirty = true;
                ctx.request_repaint();
            }
        }
        // While a frame cooks in the background, the viewport still shows the last one: say
        // so, so a click that's working doesn't look like one that did nothing.
        if self.simulation_rx.is_some() {
            let label = self.graph.cooking_label(&self.view_displays());
            let label = if label.is_empty() { "Cooking".to_owned() } else { label };
            let at = response.rect.left_top() + egui::vec2(12.0, 12.0);
            let text = format!("{label}…   Esc to cancel");
            let font = egui::FontId::proportional(14.0);
            let galley = painter.layout_no_wrap(text, font, egui::Color32::from_gray(235));
            let spinner = 16.0;
            let backing = egui::Rect::from_min_size(at, egui::vec2(galley.size().x + spinner + 26.0, galley.size().y.max(spinner) + 12.0));
            painter.rect_filled(backing, 6.0, egui::Color32::from_black_alpha(170));
            let spin = egui::Rect::from_min_size(at + egui::vec2(8.0, (backing.height() - spinner) * 0.5), egui::vec2(spinner, spinner));
            ui.put(spin, egui::Spinner::new().size(spinner).color(egui::Color32::from_gray(235)));
            painter.galley(at + egui::vec2(spinner + 16.0, 6.0), galley, egui::Color32::from_gray(235));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    // What the gizmo edits for the selected node: an object's transform, a Transform
    // SOP, or the domain box of a Pyro or FLIP solver.
    fn gizmo_target(&self) -> Option<GizmoTarget> {
        if self.context != AppContext::Obj {
            return None;
        }
        let id = self.selected?;
        let node = self.graph.nodes.get(&id)?;
        let identity = ObjectXform::default();
        // SOPs live in their geometry object's space.
        let space = self.graph.enclosing_geometry(id).map_or(identity, |object| self.graph.xform_at(object, self.time));
        let target = |value: ObjectXform, space: ObjectXform, style: GizmoStyle| Some(GizmoTarget { node: id, value, space, style });
        let boxed = |centre: Vec3, size: Vec3| ObjectXform { t: [centre.x, centre.y, centre.z], r: [0.0; 3], s: [size.x, size.y, size.z] };
        match &node.kind {
            NodeKind::Geometry { .. } => target(node.xform, identity, GizmoStyle::Xform),
            NodeKind::Light { position, rotation, .. } | NodeKind::Camera { position, rotation, .. } => {
                target(ObjectXform { t: *position, r: *rotation, s: [1.0; 3] }, identity, GizmoStyle::Xform)
            }
            NodeKind::Transform { t, r, s } => target(ObjectXform { t: *t, r: *r, s: *s }, space, GizmoStyle::Xform),
            NodeKind::Pyro(p) => target(boxed(Vec3::from_arr(p.center), Vec3::from_arr(p.size)), space, GizmoStyle::Box),
            NodeKind::Flip { domain, .. } if domain[3] > 0.0 => {
                target(boxed(Vec3::new(domain[0], domain[1], domain[2]), Vec3::new(domain[3], domain[4], domain[5])), space, GizmoStyle::Box)
            }
            NodeKind::Flip { .. } => {
                // Automatic domain: the emitter's bounds (cooked once per upstream change).
                let key = self.graph.sim_key(id);
                let cached = self.gizmo_bounds.borrow().filter(|(k, _)| *k == key).map(|(_, b)| b);
                let bounds = cached.unwrap_or_else(|| {
                    let b = self.graph.cook_fresh(self.graph.input_of(id, 0), 0.0).bounds();
                    *self.gizmo_bounds.borrow_mut() = Some((key, b));
                    b
                });
                let (lo, hi) = bounds?;
                target(boxed(lo.add(hi).scale(0.5), hi.sub(lo)), space, GizmoStyle::Box)
            }
            _ => None,
        }
    }

    // Turns the mouse motion since the drag started into new values for the target.
    fn apply_gizmo_drag(&mut self, drag: &GizmoDrag, pointer: egui::Pos2) {
        let total = pointer - drag.start_pointer;
        let frame = &drag.frame;
        let target = &drag.target;
        let start = target.value;
        let mut x = start;
        let screen_move = || {
            let ppu = frame.pixels_per_unit;
            target.local_dir(frame.cam_right.scale(total.x / ppu).sub(frame.cam_up.scale(total.y / ppu)))
        };
        match (target.style, self.gizmo_mode, drag.part.handle) {
            (GizmoStyle::Box, _, GizmoHandle::Face(i, plus)) => {
                let a = drag.part.axis_screen;
                let grow = total.dot(a) / a.length_sq().max(1e-6);
                let size = (start.s[i] + grow).max(0.01);
                x.s[i] = size;
                x.t[i] = start.t[i] + if plus { 0.5 } else { -0.5 } * (size - start.s[i]);
            }
            (GizmoStyle::Box, _, GizmoHandle::Center) | (GizmoStyle::Xform, GizmoMode::Translate, GizmoHandle::Center) => {
                let d = screen_move();
                x.t = [start.t[0] + d.x, start.t[1] + d.y, start.t[2] + d.z];
            }
            (GizmoStyle::Xform, GizmoMode::Translate, GizmoHandle::Axis(i)) => {
                let a = drag.part.axis_screen;
                x.t[i] += total.dot(a) / a.length_sq().max(1e-6);
            }
            (GizmoStyle::Xform, GizmoMode::Rotate, GizmoHandle::Ring(i)) => {
                let angle = |p: egui::Pos2| (p.y - frame.pivot.y).atan2(p.x - frame.pivot.x);
                let mut delta = angle(pointer) - angle(drag.start_pointer);
                if delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                } else if delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                x.r[i] += delta.to_degrees() * drag.part.ring_sign;
            }
            (GizmoStyle::Xform, GizmoMode::Scale, GizmoHandle::Axis(i)) => {
                let dir = drag.part.axis_screen.normalized();
                x.s[i] = (start.s[i] * (1.0 + total.dot(dir) / frame.handle_px)).max(0.001);
            }
            (GizmoStyle::Xform, GizmoMode::Scale, GizmoHandle::Center) => {
                let factor = (1.0 + (total.x - total.y) / frame.handle_px).max(0.001);
                x.s = start.s.map(|s| (s * factor).max(0.001));
            }
            _ => return,
        }
        let Some(node) = self.graph.nodes.get_mut(&frame.node) else { return };
        match &mut node.kind {
            NodeKind::Geometry { .. } => node.xform = x,
            NodeKind::Light { position, rotation, .. } | NodeKind::Camera { position, rotation, .. } => {
                *position = x.t;
                *rotation = x.r;
            }
            NodeKind::Transform { t, r, s } => {
                *t = x.t;
                *r = x.r;
                *s = x.s;
            }
            NodeKind::Pyro(p) => {
                p.center = x.t;
                p.size = x.s;
            }
            NodeKind::Flip { domain, .. } => *domain = [x.t[0], x.t[1], x.t[2], x.s[0], x.s[1], x.s[2]],
            _ => return,
        }
        self.dirty = true;
    }
}

// ============================================================================
// 7. UPDATE LOOP
// ============================================================================

impl eframe::App for ProceduralFxStudioApp {
    // A clean quit (saved, or "Don't Save"): nothing to recover next time.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(session) = &mut self.recovery {
            session.end();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A dialog in its own window locks this one.
        lock_for_modal(ctx);
        // Windows rendering set-up: show what the helper is doing.
        if let Some(rx) = &self.render_setup {
            let mut lines = Vec::new();
            let mut finished = false;
            loop {
                match rx.try_recv() {
                    Ok(line) => lines.push(line),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if let Some(line) = lines.last() {
                self.status = wslrender::progress(line).unwrap_or_else(|| line.trim_start_matches("[pfx-render] ").replacen("error ", "Error: ", 1));
            }
            for line in lines {
                if line == "[pfx-render] needs-admin" {
                    self.render_setup_needs_admin = true;
                    self.status = "Rendering on Windows needs WSL: see Set Up Rendering on Windows".into();
                    continue;
                }
                // Download progress replaces the previous progress line.
                let progress = line.starts_with("[pfx-render] download ");
                if progress && self.render_setup_log.last().is_some_and(|l| l.starts_with("[pfx-render] download ")) {
                    self.render_setup_log.pop();
                }
                self.render_setup_log.push(line);
            }
            if finished {
                self.render_setup = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if self.render_setup_open {
            let mut open = true;
            let running = self.render_setup.is_some();
            own_window(ctx, "render_setup", "Set Up Rendering on Windows", [500.0, 420.0], [400.0, 300.0], &mut open, true, |ui| {
                ui.label("Installs WSL containers if they're missing, then downloads MoonRay (about 1.3 GB, once).");
                ui.separator();
                egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.render_setup_log {
                        let text = wslrender::progress(line).unwrap_or_else(|| line.trim_start_matches("[pfx-render] ").to_string());
                        if line.contains("] error ") {
                            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), text.replacen("error ", "", 1));
                        } else {
                            ui.label(text);
                        }
                    }
                });
                if running {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Working… a Windows permission prompt may appear.");
                    });
                }
                let mut next = None;
                if self.render_setup_needs_admin && !running {
                    ui.separator();
                    ui.label(egui::RichText::new("This PC needs WSL (the Windows Subsystem for Linux) to render.").strong());
                    ui.label("Installing it needs an administrator, once. Turning on Virtual Machine Platform also needs a restart. After that, downloading MoonRay and rendering work without administrator rights.");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Install now").on_hover_text("Windows asks for an administrator's approval").clicked() {
                            next = Some(true);
                        }
                        if ui.button("Copy the command for an administrator").on_hover_text(format!("Run in a Terminal opened as administrator: {WSL_ADMIN_COMMAND}")).clicked() {
                            ui.ctx().copy_text(WSL_ADMIN_COMMAND.to_string());
                            self.render_setup_log.push(format!("[pfx-render] status copied: {WSL_ADMIN_COMMAND} (an administrator runs it once in a Terminal opened as administrator, then restarts)"));
                        }
                        if ui.button("Check again").on_hover_text("After WSL was installed (and Windows restarted)").clicked() {
                            next = Some(false);
                        }
                    });
                }
                if let (Some(install), Some(helper)) = (next, wslrender::helper()) {
                    self.render_setup_needs_admin = false;
                    self.render_setup = Some(start_render_setup(helper, install));
                }
            });
            if !open {
                self.render_setup_open = false;
            }
        }
        let dt = ctx.input(|i| i.unstable_dt);
        if dt.is_finite() && dt > 0.0 && dt < 1.0 {
            self.frame_time += (dt - self.frame_time) * 0.1;
        }
        self.shortcuts(ctx);

        // Keep the requested simulation time stable until the asynchronous
        // FLIP cook completes. Otherwise playback changes the cache key every
        // frame and cancels the job before it can produce a result.
        if self.animate && self.simulation_rx.is_none() {
            // Whole frames at the playback rate. A slow cook holds playback back
            // rather than making it skip frames, so at most one frame is owed.
            if dt.is_finite() && dt > 0.0 {
                self.playback_due = (self.playback_due + dt.min(0.25) * self.playback_fps.clamp(1.0, 120.0)).min(1.0);
            }
            if self.playback_due >= 1.0 {
                self.playback_due -= 1.0;
                let frame = (self.time * sims::SIM_FPS).round() + 1.0;
                self.time = frame / sims::SIM_FPS;
                if self.time > self.playback_end + 1e-4 {
                    self.time = self.playback_start;
                }
                self.dirty = true;
            }
            ctx.request_repaint();
        }

        self.begin_animation_frame(ctx);
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.toolbar(ui));
        self.updater.poll(ctx);
        if let Some((id, target)) = self.group_pick_request.take() {
            self.start_group_pick(id, target);
        }
        if let Some(domain) = self.group_pick_domain.take() {
            self.set_group_pick_domain(domain);
        }
        if let Some(accept) = self.group_pick_finish.take() {
            self.finish_group_pick(accept);
        }
        if self.group_pick.is_some() {
            let (enter, escape) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter || escape {
                self.finish_group_pick(enter);
            }
            // Picking is tied to the node it was started from.
            if self.group_pick.as_ref().is_some_and(|p| Some(p.node) != self.selected) {
                self.finish_group_pick(false);
            }
        }
        if let Some(id) = self.group_choices_request.take() {
            let input = self.graph.input_of(id, 0).map(|i| self.graph.cook(i, self.time, &mut HashMap::new())).unwrap_or_default();
            let (points, prims) = groups::names(&input);
            self.group_choices = Some((id, points, prims));
        }
        if let Some(id) = self.paint_fill.take() {
            let count = self.graph.input_of(id, 0).map_or(0, |i| self.graph.cook(i, self.time, &mut HashMap::new()).vertices.len());
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                *values = vec![settings.value; count];
                self.dirty = true;
            }
        }
        if self.updater.show(ctx) == updater::UpdaterAction::SaveScene {
            self.save_scene();
        }
        self.update_window_title(ctx);
        self.confirm_quit(ctx);
        self.autosave(ctx);
        self.recovery_prompt(ctx);

        self.poll_moonray_render();
        self.advance_sequence();
        if let Some(id) = self.top_cook_request.take() {
            self.start_top_cook(id);
        }
        if self.top_run.is_some() || self.top_cancel_request {
            self.advance_top_run();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some((id, range)) = self.cop_write_request.take() {
            self.status = self.write_cop_output(id, range).unwrap_or_else(|e| format!("File Output failed: {e}"));
        }

        self.live_sheet_step(ctx);
        self.flipbook_step(ctx);
        if self.dirty || self.simulation_rx.is_some() {
            self.rebuild_graph();
            self.dirty = false;
        }
        self.flipbook_request_capture(ctx);
        if self.simulation_rx.is_some() || self.moonray_rx.is_some() || self.sequence.is_some() {
            ctx.request_repaint();
        }

        // The panes: a dock the user can rearrange, split, float and pop out (see Pane).
        self.show_panes(ctx);
        if let Some((scene, preset)) = self.assistant.show(ctx, &self.graph, self.time) {
            self.graph = scene;
            self.dirty = true;
            self.status = "AI changes applied (Ctrl+Z undoes them)".into();
            // Started from a preset: its range, sheet settings and (newest) camera come too.
            if let Some(preset) = preset {
                let camera = self.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. }) && n.name == "sheet_camera").map(|(id, _)| *id).max();
                self.use_preset_settings(preset, camera);
            }
        }
        if let Some(viewer) = self.flipbook_viewer.as_mut() {
            viewer.show(ctx);
            if !viewer.open {
                self.flipbook_viewer = None;
            }
        }
        if self.dirty {
            ctx.request_repaint();
        }

        self.use_requested_camera();
        if let Some((id, range)) = self.file_export_request.take() {
            self.status = self.write_file_export(id, range).unwrap_or_else(|e| format!("File Export failed: {e}"));
        }
        if let Some(id) = self.usd_export_request.take() {
            if let Some(NodeKind::LopExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) {
                let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
                self.status = match usd::save(&stage, file.trim()) {
                    Ok(()) => format!("Saved USD ({} prims) to {}", stage.prims.len(), file.trim()),
                    Err(e) => format!("USD export failed: {e}"),
                };
            }
        }
        if let Some(id) = self.pyro_fit_request.take() {
            let source = self.graph.cook_fresh(self.graph.input_of(id, 0), 0.0);
            match (source.bounds(), self.graph.nodes.get_mut(&id).map(|n| &mut n.kind)) {
                (Some((lo, hi)), Some(NodeKind::Pyro(set))) => {
                    let extent = hi.sub(lo);
                    let width = (extent.x.max(extent.z) * 3.0).max(2.0);
                    let height = (extent.y * 2.0 + width * 1.5).max(3.0);
                    set.size = [width.max(extent.x * 3.0), height, width.max(extent.z * 3.0)];
                    set.center = [(lo.x + hi.x) * 0.5, lo.y - 0.1 * height + height * 0.5, (lo.z + hi.z) * 0.5];
                    self.dirty = true;
                }
                _ => self.status = "Pyro: connect a source to input 1 first".into(),
            }
        }
        if let Some(id) = self.loop_search_request.take() {
            if let Some(&NodeKind::MakeLoop { start, length, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
                let (best, diff) = looping::best_length(|t| self.graph.cook_input_fresh(id, 0, t), start, length * 0.5, length * 2.0, length);
                if let Some(NodeKind::MakeLoop { length, blend, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                    *length = best;
                    *blend = blend.min(best);
                }
                self.playback_end = start + best - 1.0 / sims::SIM_FPS;
                self.dirty = true;
                self.status = format!("Make Loop: {} frames ({best:.2} s) loops best, its end {:.0}% different from its start. The playback range now ends there.", (best * sims::SIM_FPS).round(), diff * 100.0);
            }
        }
        if self.moonray_render_requested {
            self.moonray_render_requested = false;
            self.start_moonray_render();
        }

        self.render_settings_window(ctx);
        self.sprite_sheet_window(ctx);
        self.vat_window(ctx);
        self.viewer.ui(ctx);


        // Node Info: a pinned window, and the popup while the middle button is held on a node.
        if let Some(node_id) = self.inspect_node.filter(|id| self.graph.nodes.contains_key(id)) {
            let mut open = true;
            let title = self.graph.nodes[&node_id].title(node_id);
            own_window(ctx, "node_info_window", &format!("Node Info · {title}"), [380.0, 560.0], [280.0, 200.0], &mut open, false, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| self.node_info_ui(ui, node_id));
            });
            if !open {
                self.inspect_node = None;
            }
        } else {
            self.inspect_node = None;
        }
        if let Some((node_id, at)) = self.info_hold {
            if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) || !self.graph.nodes.contains_key(&node_id) {
                self.info_hold = None;
            } else {
                egui::Area::new(egui::Id::new("node_info_hold")).order(egui::Order::Tooltip).fixed_pos(at + egui::vec2(14.0, 14.0)).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(380.0);
                        self.node_info_ui(ui, node_id);
                    });
                });
            }
        }

        self.end_animation_frame();

        // Pause on edit: any change to the scene (parameters, wiring, flags, gizmo drags, undo)
        // stops playback, not only edits in the parameter panel.
        let signature = self.graph.content_signature(self.active_camera);
        if self.last_content_signature.is_some_and(|last| last != signature) && self.pause_on_edit && self.animate {
            self.animate = false;
            self.status = "Playback paused: the scene was edited (turn off Pause on edit to keep playing)".into();
        }
        self.last_content_signature = Some(signature);

        // Undo snapshots: commit once the mouse is released (one step per drag)
        if !ctx.input(|i| i.pointer.any_down()) {
            let s = self.graph.serialize();
            if s != self.last_state {
                self.undo.push(std::mem::replace(&mut self.last_state, s));
                self.redo.clear();
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
            }
        }
        // A dialog opened in its own window this frame: cover this one until it closes.
        if let Some((_, title)) = ctx.data(|d| d.get_temp::<(egui::ViewportId, String)>(modal_key())) {
            modal_cover(ctx, &title);
        }
    }
}

fn main() -> eframe::Result<()> {
    // Windows: USD finds its own plugins in usd\ beside its DLLs, but not the plugin
    // libraries in plugin\usd, which the app ships next to itself.
    if cfg!(windows) {
        if let Some(plugins) = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("plugin").join("usd"))).filter(|dir| dir.is_dir()) {
            let mut path = plugins.into_os_string();
            if let Some(existing) = std::env::var_os("PXR_PLUGINPATH_NAME").filter(|p| !p.is_empty()) {
                path.push(";");
                path.push(existing);
            }
            std::env::set_var("PXR_PLUGINPATH_NAME", path);
        }
    }
    // `--usd-info <file>` lists a USD file's prims and exits (checks USD works in a packaged app).
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == "--usd-info") {
        let Some(file) = args.get(at + 1) else {
            eprintln!("usage: --usd-info <file.usd>");
            std::process::exit(2);
        };
        match usd::load_file(file, 0.0) {
            Ok(stage) => {
                for (path, prim) in &stage.prims {
                    println!("{path} {}", prim.type_name);
                }
                println!("{} prims", stage.prims.len());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // `render` and `sprite-sheet` render a scene without a window, then exit.
    if let Some(code) = cli::main(&args) {
        std::process::exit(code);
    }
    recovery::install_panic_hook();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("ProceduralFX Studio")
            .with_icon(app_icon()),
        // macOS: winit's default app menu quits on Cmd+Q without asking the window,
        // which would skip "Save changes before quitting?"; the app handles Cmd+Q itself.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
        ..Default::default()
    };
    // wgpu (Metal, DirectX 12, Vulkan) unless PFX_RENDERER=opengl. If wgpu can't
    // start (no usable GPU or driver) before the app opens, run again with OpenGL.
    let opengl = std::env::var("PFX_RENDERER").is_ok_and(|r| r.eq_ignore_ascii_case("opengl"));
    let opened = Arc::new(AtomicBool::new(false));
    let run = |renderer: eframe::Renderer, options: eframe::NativeOptions| {
        let opened = opened.clone();
        eframe::run_native(
            "ProceduralFX Studio",
            eframe::NativeOptions { renderer, ..options },
            Box::new(move |cc| {
                opened.store(true, Ordering::SeqCst);
                let mut app = ProceduralFxStudioApp::default();
                if let Some(dir) = recovery_dir() {
                    let (session, leftovers) = recovery::Session::start(dir);
                    app.recovery = Some(session);
                    app.recovery_offer = leftovers.into_iter().next();
                }
                // PFX_VIEWPORT=cpu keeps the viewport on raster.rs (to compare the two).
                let cpu = std::env::var("PFX_VIEWPORT").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
                app.gpu_viewport = cc.wgpu_render_state.clone().filter(|_| !cpu).map(gpu_viewport::GpuViewport::new);
                Ok(Box::new(app))
            }),
        )
    };
    if opengl {
        return run(eframe::Renderer::Glow, options);
    }
    match run(eframe::Renderer::Wgpu, options.clone()) {
        Err(eframe::Error::Wgpu(error)) if !opened.load(Ordering::SeqCst) => {
            eprintln!("wgpu couldn't start ({error}); using OpenGL");
            run(eframe::Renderer::Glow, options)
        }
        result => result,
    }
}

#[cfg(test)]
mod app_tests;
