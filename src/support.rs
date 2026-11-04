// Cooking state and native helpers: moved out of main.rs.

use crate::*;

thread_local! {
    // Previous step's geometry for each SOP Solver being stepped on this thread.
    pub(crate) static SOLVER_PREV_FRAME: std::cell::RefCell<HashMap<NodeId, Mesh>> = std::cell::RefCell::new(HashMap::new());
    // @TimeInc for wrangles: one frame, or one step while a SOP Solver is stepping.
    pub(crate) static SOLVER_TIME_INC: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0 / sims::SIM_FPS) };
    // The geometry each For-Each Begin puts out for the iteration being cooked on this thread.
    pub(crate) static FOREACH_INPUT: std::cell::RefCell<HashMap<NodeId, Mesh>> = std::cell::RefCell::new(HashMap::new());
    // Object Merge nesting, to stop merges that (indirectly) merge themselves.
    pub(crate) static MERGE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static FLIP_CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn flip_cancelled() -> bool {
    FLIP_CANCEL.with(|flag| {
        flag.borrow()
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    })
}

pub(crate) fn make_png_opaque(path: &std::path::Path) -> Result<(), String> {
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

pub(crate) fn ensure_png_not_all_black(path: &std::path::Path) -> Result<bool, String> {
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
    pub(crate) fn mini_openvdb_surface(
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
    pub(crate) fn mini_openvdb_free(pointer: *mut std::ffi::c_void);
    pub(crate) fn mini_openvdb_volume_from_mesh(
        vertices: *const f32,
        vertex_count: usize,
        triangles: *const u32,
        triangle_count: usize,
        voxel_size: f32,
        padding: u32,
    ) -> *mut std::ffi::c_void;
    pub(crate) fn mini_openvdb_volume_clone(handle: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    pub(crate) fn mini_openvdb_volume_operation(
        handle: *mut std::ffi::c_void,
        operation: u32,
        iterations: u32,
    ) -> i32;
    pub(crate) fn mini_openvdb_volume_to_mesh(
        handle: *mut std::ffi::c_void,
        out_vertices: *mut *mut f32,
        out_vertex_count: *mut usize,
        out_triangles: *mut *mut u32,
        out_triangle_count: *mut usize,
    ) -> i32;
    pub(crate) fn mini_openvdb_volume_free(handle: *mut std::ffi::c_void);
}

#[cfg(feature = "native-openvdb")]
pub(crate) struct NativeVolume(pub(crate) *mut std::ffi::c_void);

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
