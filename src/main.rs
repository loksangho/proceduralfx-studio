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
mod app;
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
    // Menu bar for occasional commands, plus one row of the controls used all the time.
    // The menus, also reachable from one ☰ button when the window is too narrow for them.
    const MENUS: [&'static str; 7] = ["File", "Edit", "Select", "View", "Render", "Windows", "Help"];

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
