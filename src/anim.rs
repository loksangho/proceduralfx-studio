// Keyframe animation: channels of keys with constant, linear, smooth (automatic
// tangents) or bezier (editable tangents) interpolation, plus the Graph Editor.
//
// Times are in seconds, like the timeline; the editor shows frames at SIM_FPS.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Interp {
    Constant,
    Linear,
    // Smooth curve with automatic, overshoot-free tangents.
    Auto,
    // Smooth curve with the key's own tangents (edited in the Graph Editor).
    Bezier,
}

impl Interp {
    pub const ALL: [(Interp, &'static str); 4] =
        [(Interp::Constant, "Constant"), (Interp::Linear, "Linear"), (Interp::Auto, "Smooth"), (Interp::Bezier, "Bezier")];

    fn code(self) -> u8 {
        match self {
            Interp::Constant => 0,
            Interp::Linear => 1,
            Interp::Auto => 2,
            Interp::Bezier => 3,
        }
    }

    fn from_code(code: u8) -> Interp {
        match code {
            0 => Interp::Constant,
            1 => Interp::Linear,
            3 => Interp::Bezier,
            _ => Interp::Auto,
        }
    }
}

// `interp` shapes the segment leaving this key; the slopes are used by Bezier keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub t: f32,
    pub v: f32,
    pub interp: Interp,
    pub in_slope: f32,
    pub out_slope: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Channel {
    // Sorted by time.
    pub keys: Vec<Key>,
    // When set, the parameter follows this expression instead of the keys
    // (VEX, with $F, $FF, $T and $FPS; see vex::eval_expression).
    pub expression: Option<String>,
}

impl Channel {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.expression.is_none()
    }

    // The value at `t`: the expression's, else the keys'. None when there's
    // neither, or the expression doesn't evaluate (the parameter keeps its own value).
    pub fn value(&self, t: f32) -> Option<f32> {
        match &self.expression {
            Some(expr) => crate::vex::eval_expression(expr, t).ok(),
            None => (!self.keys.is_empty()).then(|| self.eval(t)),
        }
    }

    pub fn constant(t: f32, v: f32) -> Channel {
        let mut channel = Channel::default();
        channel.set_key(t, v);
        channel
    }

    fn index_at(&self, t: f32) -> Option<usize> {
        self.keys.iter().position(|k| (k.t - t).abs() < 1e-4)
    }

    // Adds a key, or changes the value of the key already at `t`.
    pub fn set_key(&mut self, t: f32, v: f32) -> usize {
        if let Some(i) = self.index_at(t) {
            self.keys[i].v = v;
            return i;
        }
        let interp = self.keys.iter().rev().find(|k| k.t < t).or(self.keys.first()).map_or(Interp::Auto, |k| k.interp);
        let at = self.keys.partition_point(|k| k.t < t);
        self.keys.insert(at, Key { t, v, interp, in_slope: 0.0, out_slope: 0.0 });
        at
    }

    pub fn remove_key_at(&mut self, t: f32) -> bool {
        match self.index_at(t) {
            Some(i) => {
                self.keys.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn has_key_at(&self, t: f32) -> bool {
        self.index_at(t).is_some()
    }

    pub fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }

    // Slope of a smooth key: Catmull-Rom, flattened at peaks and dips so curves never overshoot.
    fn auto_slope(&self, i: usize) -> f32 {
        let k = &self.keys[i];
        let (Some(prev), Some(next)) = (i.checked_sub(1).map(|p| &self.keys[p]), self.keys.get(i + 1)) else {
            return 0.0;
        };
        if (k.v - prev.v) * (next.v - k.v) <= 0.0 {
            return 0.0;
        }
        (next.v - prev.v) / (next.t - prev.t).max(1e-6)
    }

    // (incoming, outgoing) slopes of key `i`.
    pub fn slopes(&self, i: usize) -> (f32, f32) {
        let k = &self.keys[i];
        match k.interp {
            Interp::Bezier => (k.in_slope, k.out_slope),
            _ => {
                let s = self.auto_slope(i);
                (s, s)
            }
        }
    }

    pub fn eval(&self, t: f32) -> f32 {
        let keys = &self.keys;
        match keys.len() {
            0 => return 0.0,
            1 => return keys[0].v,
            _ => {}
        }
        if t <= keys[0].t {
            return keys[0].v;
        }
        let last = keys.len() - 1;
        if t >= keys[last].t {
            return keys[last].v;
        }
        let i = keys.partition_point(|k| k.t <= t) - 1;
        let (k0, k1) = (&keys[i], &keys[i + 1]);
        let h = (k1.t - k0.t).max(1e-6);
        let s = (t - k0.t) / h;
        match k0.interp {
            Interp::Constant => k0.v,
            Interp::Linear => k0.v + (k1.v - k0.v) * s,
            Interp::Auto | Interp::Bezier => {
                let m0 = self.slopes(i).1 * h;
                // A linear or constant key at the far end is approached in a straight line.
                let m1 = match k1.interp {
                    Interp::Linear | Interp::Constant if k0.interp == Interp::Auto => k1.v - k0.v,
                    _ => self.slopes(i + 1).0 * h,
                };
                let (s2, s3) = (s * s, s * s * s);
                (2.0 * s3 - 3.0 * s2 + 1.0) * k0.v + (s3 - 2.0 * s2 + s) * m0 + (-2.0 * s3 + 3.0 * s2) * k1.v + (s3 - s2) * m1
            }
        }
    }

    // `t,v,interp,in,out;...`
    // Keys separated by ';'; an expression goes first as "expr:" and its UTF-8 in hex.
    pub fn to_text(&self) -> String {
        let expression = self.expression.iter().map(|e| format!("expr:{}", e.bytes().map(|b| format!("{b:02x}")).collect::<String>()));
        expression
            .chain(self.keys.iter().map(|k| format!("{},{},{},{},{}", k.t, k.v, k.interp.code(), k.in_slope, k.out_slope)))
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn from_text(text: &str) -> Channel {
        let mut channel = Channel::default();
        for key in text.split(';') {
            if let Some(hex) = key.strip_prefix("expr:") {
                let bytes: Option<Vec<u8>> = (0..hex.len() / 2).map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
                channel.expression = bytes.and_then(|b| String::from_utf8(b).ok());
                continue;
            }
            let f: Vec<f32> = key.split(',').filter_map(|v| v.parse().ok()).collect();
            if f.len() == 5 && f[0].is_finite() && f[1].is_finite() {
                channel.keys.push(Key { t: f[0], v: f[1], interp: Interp::from_code(f[2] as u8), in_slope: f[3], out_slope: f[4] });
            }
        }
        channel.sort();
        channel
    }
}

// ============================================================================
// Graph Editor
// ============================================================================

// A channel as the editor lists it.
pub struct ChannelRow<'a> {
    pub label: String,
    pub group: String,
    pub channel: &'a mut Channel,
}

pub struct GraphEditor {
    pub open: bool,
    // Visible window: time (seconds) and value ranges.
    t0: f32,
    t1: f32,
    v0: f32,
    v1: f32,
    needs_frame: bool,
    snap: bool,
    // Selected keys as (row index, key index); refreshed when rows change.
    selected: Vec<(usize, usize)>,
    hidden: std::collections::HashSet<String>,
    drag: Option<Drag>,
    row_signature: Vec<String>,
}

#[derive(Clone)]
enum Drag {
    Keys { start: egui::Pos2, original: Vec<(usize, usize, f32, f32)> },
    Tangent { row: usize, key: usize, outgoing: bool },
    Box { start: egui::Pos2 },
    Scrub,
}

impl Default for GraphEditor {
    fn default() -> Self {
        Self {
            open: false,
            t0: 0.0,
            t1: 5.0,
            v0: -1.0,
            v1: 1.0,
            needs_frame: true,
            snap: true,
            selected: Vec::new(),
            hidden: Default::default(),
            drag: None,
            row_signature: Vec::new(),
        }
    }
}

const COLOURS: [egui::Color32; 6] = [
    egui::Color32::from_rgb(235, 90, 90),
    egui::Color32::from_rgb(110, 210, 110),
    egui::Color32::from_rgb(100, 150, 245),
    egui::Color32::from_rgb(230, 190, 80),
    egui::Color32::from_rgb(200, 120, 220),
    egui::Color32::from_rgb(90, 205, 205),
];

pub fn channel_colour(label: &str) -> egui::Color32 {
    if label.ends_with('x') || label.ends_with(".r") || label.ends_with("[0]") {
        COLOURS[0]
    } else if label.ends_with('y') || label.ends_with(".g") || label.ends_with("[1]") {
        COLOURS[1]
    } else if label.ends_with('z') || label.ends_with(".b") || label.ends_with("[2]") {
        COLOURS[2]
    } else {
        let hash = label.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        COLOURS[3 + (hash as usize % 3)]
    }
}

pub struct EditorResult {
    // New current time when the user scrubbed in the editor.
    pub scrub: Option<f32>,
    pub changed: bool,
}

impl GraphEditor {
    pub fn frame_all(&mut self) {
        self.needs_frame = true;
    }

    fn fit(&mut self, rows: &[ChannelRow], time_range: (f32, f32)) {
        let mut t = (f32::INFINITY, f32::NEG_INFINITY);
        let mut v = (f32::INFINITY, f32::NEG_INFINITY);
        for row in rows.iter().filter(|r| !self.hidden.contains(&r.label)) {
            for k in &row.channel.keys {
                t = (t.0.min(k.t), t.1.max(k.t));
                v = (v.0.min(k.v), v.1.max(k.v));
            }
        }
        if !t.0.is_finite() {
            t = time_range;
            v = (-1.0, 1.0);
        }
        let (tp, vp) = (((t.1 - t.0) * 0.1).max(0.25), ((v.1 - v.0) * 0.15).max(0.25));
        (self.t0, self.t1, self.v0, self.v1) = (t.0 - tp, t.1 + tp, v.0 - vp, v.1 + vp);
        self.needs_frame = false;
    }

    // The editor drawn into `ui` (a dock pane); the caller decides where it lives.
    pub fn ui_inside(&mut self, ui: &mut egui::Ui, rows: &mut [ChannelRow], time: f32, time_range: (f32, f32), fps: f32) -> EditorResult {
        let mut result = EditorResult { scrub: None, changed: false };
        // Key selection indexes rows, so a different set of channels clears it.
        let signature: Vec<String> = rows.iter().map(|r| r.label.clone()).collect();
        if signature != self.row_signature {
            self.row_signature = signature;
            self.selected.clear();
            self.needs_frame = true;
        }
        if self.needs_frame {
            self.fit(rows, time_range);
        }
        self.toolbar(ui, rows, &mut result);
        ui.separator();
        ui.horizontal_top(|ui| {
            egui::ScrollArea::vertical().id_salt("graph_channels").max_width(170.0).min_scrolled_height(ui.available_height()).show(ui, |ui| {
                ui.set_width(160.0);
                if rows.is_empty() {
                    ui.weak("No animated channels. Select a node and press K or ◆ Key, or turn on Auto Key.");
                }
                let mut group = "";
                for row in rows.iter() {
                    if row.group != group {
                        group = &row.group;
                        ui.strong(group);
                    }
                    let mut visible = !self.hidden.contains(&row.label);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, channel_colour(&row.label));
                        if ui.checkbox(&mut visible, format!("{}  ({})", short_label(&row.label), row.channel.keys.len())).changed() {
                            if visible {
                                self.hidden.remove(&row.label);
                            } else {
                                self.hidden.insert(row.label.clone());
                            }
                        }
                    });
                }
            });
            ui.separator();
            self.canvas(ui, rows, time, fps, &mut result);
        });
        result
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, rows: &mut [ChannelRow], result: &mut EditorResult) {
        ui.horizontal_wrapped(|ui| {
            let any = !self.selected.is_empty();
            for (interp, label) in Interp::ALL {
                if ui.add_enabled(any, egui::Button::new(label)).on_hover_text("Interpolation of the selected keys").clicked() {
                    self.set_interp(rows, interp);
                    result.changed = true;
                }
            }
            if ui.add_enabled(any, egui::Button::new("Flatten")).on_hover_text("Flat tangents on the selected keys").clicked() {
                for &(r, k) in &self.selected {
                    if let Some(key) = rows.get_mut(r).and_then(|row| row.channel.keys.get_mut(k)) {
                        key.interp = Interp::Bezier;
                        (key.in_slope, key.out_slope) = (0.0, 0.0);
                    }
                }
                result.changed = true;
            }
            if ui.add_enabled(any, egui::Button::new("Delete keys")).clicked() {
                self.delete_selected(rows);
                result.changed = true;
            }
            ui.separator();
            ui.checkbox(&mut self.snap, "Snap to frames");
            if ui.button("Frame all (F)").clicked() {
                self.needs_frame = true;
            }
            ui.weak("Drag keys • Shift-click add • drag empty: box • double-click: add key • wheel: zoom (Shift: value) • right-drag: pan");
        });
    }

    fn set_interp(&mut self, rows: &mut [ChannelRow], interp: Interp) {
        for &(r, k) in &self.selected {
            let Some(row) = rows.get_mut(r) else { continue };
            if k >= row.channel.keys.len() {
                continue;
            }
            // Switching to Bezier starts from the current smooth tangents.
            let slopes = row.channel.slopes(k);
            let key = &mut row.channel.keys[k];
            if interp == Interp::Bezier && key.interp != Interp::Bezier {
                (key.in_slope, key.out_slope) = slopes;
            }
            key.interp = interp;
        }
    }

    fn delete_selected(&mut self, rows: &mut [ChannelRow]) {
        let mut selected = std::mem::take(&mut self.selected);
        selected.sort_unstable_by(|a, b| b.cmp(a));
        for (r, k) in selected {
            if let Some(row) = rows.get_mut(r) {
                if k < row.channel.keys.len() {
                    row.channel.keys.remove(k);
                }
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui, rows: &mut [ChannelRow], time: f32, fps: f32, result: &mut EditorResult) {
        let (response, painter) = ui.allocate_painter(ui.available_size().max(egui::vec2(200.0, 120.0)), egui::Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(26));
        let ruler = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + 18.0));
        let (t0, t1, v0, v1) = (self.t0, self.t1, self.v0, self.v1);
        let to_x = |t: f32| rect.min.x + (t - t0) / (t1 - t0) * rect.width();
        let to_y = |v: f32| rect.max.y - (v - v0) / (v1 - v0) * (rect.height() - ruler.height());
        let to_t = |x: f32| t0 + (x - rect.min.x) / rect.width() * (t1 - t0);
        let to_v = |y: f32| v0 + (rect.max.y - y) / (rect.height() - ruler.height()) * (v1 - v0);
        let snapping = self.snap;
        let snap = move |t: f32| if snapping { (t * fps).round() / fps } else { t };

        // Grid: frame lines with adaptive spacing, value lines at 1-2-5 steps.
        let frames_per_px = (t1 - t0) * fps / rect.width();
        let frame_step = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0].into_iter().find(|s| s / frames_per_px >= 40.0).unwrap_or(1000.0);
        let mut frame = ((t0 * fps) / frame_step).floor() * frame_step;
        while frame <= t1 * fps {
            let x = to_x(frame / fps);
            painter.line_segment([egui::pos2(x, ruler.max.y), egui::pos2(x, rect.max.y)], egui::Stroke::new(1.0, egui::Color32::from_gray(40)));
            painter.text(egui::pos2(x + 2.0, rect.min.y + 2.0), egui::Align2::LEFT_TOP, format!("{}", frame as i64 + 1), egui::FontId::monospace(10.0), egui::Color32::from_gray(140));
            frame += frame_step;
        }
        let value_range = v1 - v0;
        let raw_step = value_range / 6.0;
        let magnitude = 10f32.powf(raw_step.log10().floor());
        let value_step = [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * magnitude).find(|s| *s >= raw_step).unwrap_or(raw_step);
        let mut value = (v0 / value_step).floor() * value_step;
        while value <= v1 {
            let y = to_y(value);
            let colour = if value.abs() < value_step * 0.01 { egui::Color32::from_gray(70) } else { egui::Color32::from_gray(40) };
            painter.line_segment([egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)], egui::Stroke::new(1.0, colour));
            painter.text(egui::pos2(rect.min.x + 3.0, y - 1.0), egui::Align2::LEFT_BOTTOM, format!("{value:.3}"), egui::FontId::monospace(10.0), egui::Color32::from_gray(120));
            value += value_step;
        }
        painter.rect_filled(ruler, 0.0, egui::Color32::from_black_alpha(90));

        // Curves.
        for row in rows.iter().filter(|r| !self.hidden.contains(&r.label)) {
            if row.channel.keys.is_empty() {
                continue;
            }
            let colour = channel_colour(&row.label);
            let points: Vec<egui::Pos2> = (0..=rect.width() as usize / 2)
                .map(|i| {
                    let x = rect.min.x + i as f32 * 2.0;
                    egui::pos2(x, to_y(row.channel.eval(to_t(x))))
                })
                .collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.6, colour)));
        }

        // Keys and tangent handles; collect their screen positions for picking.
        let tangent_len = 40.0;
        let mut key_points: Vec<(usize, usize, egui::Pos2)> = Vec::new();
        let mut handles: Vec<(usize, usize, bool, egui::Pos2)> = Vec::new();
        for (r, row) in rows.iter().enumerate().filter(|(_, r)| !self.hidden.contains(&r.label)) {
            for (k, key) in row.channel.keys.iter().enumerate() {
                let p = egui::pos2(to_x(key.t), to_y(key.v));
                let selected = self.selected.contains(&(r, k));
                if selected && key.interp == Interp::Bezier {
                    let (slope_in, slope_out) = row.channel.slopes(k);
                    for (outgoing, slope) in [(false, slope_in), (true, slope_out)] {
                        // Handle direction in screen space for this slope.
                        let d = egui::vec2(rect.width() / (t1 - t0), -slope * (rect.height() - ruler.height()) / (v1 - v0));
                        let tip = p + d.normalized() * tangent_len * if outgoing { 1.0 } else { -1.0 };
                        painter.line_segment([p, tip], egui::Stroke::new(1.0, egui::Color32::from_gray(190)));
                        painter.circle_filled(tip, 3.5, egui::Color32::from_gray(220));
                        handles.push((r, k, outgoing, tip));
                    }
                }
                let size = if selected { 5.5 } else { 4.5 };
                let fill = if selected { egui::Color32::from_rgb(255, 225, 70) } else { channel_colour(&row.label) };
                painter.add(egui::Shape::convex_polygon(
                    vec![p + egui::vec2(0.0, -size), p + egui::vec2(size, 0.0), p + egui::vec2(0.0, size), p + egui::vec2(-size, 0.0)],
                    fill,
                    egui::Stroke::new(1.0, egui::Color32::BLACK),
                ));
                key_points.push((r, k, p));
            }
        }
        // Current time.
        let x = to_x(time);
        painter.line_segment([egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)], egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 80, 80)));

        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let shift = ui.input(|i| i.modifiers.shift);
        let nearest_key = |p: egui::Pos2| {
            key_points
                .iter()
                .map(|(r, k, kp)| (*r, *k, kp.distance(p)))
                .filter(|(_, _, d)| *d < 8.0)
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(r, k, _)| (r, k))
        };

        // Zoom and pan.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if let Some(p) = response.hover_pos() {
                    let factor = (-scroll * 0.003).exp();
                    if shift {
                        let v = to_v(p.y);
                        (self.v0, self.v1) = (v + (self.v0 - v) * factor, v + (self.v1 - v) * factor);
                    } else {
                        let t = to_t(p.x);
                        (self.t0, self.t1) = (t + (self.t0 - t) * factor, t + (self.t1 - t) * factor);
                    }
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::F)) && !ui.ctx().wants_keyboard_input() {
                self.needs_frame = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && !ui.ctx().wants_keyboard_input() && !self.selected.is_empty() {
                self.delete_selected(rows);
                result.changed = true;
            }
        }
        if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
            let d = response.drag_delta();
            let dt = d.x / rect.width() * (t1 - t0);
            let dv = d.y / (rect.height() - ruler.height()) * (v1 - v0);
            (self.t0, self.t1, self.v0, self.v1) = (self.t0 - dt, self.t1 - dt, self.v0 + dv, self.v1 + dv);
        }

        // Start of a primary drag: ruler scrub, tangent handle, key, or box.
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = pointer {
                let handle = handles.iter().find(|(_, _, _, tip)| tip.distance(p) < 6.0);
                self.drag = if ruler.contains(p) {
                    Some(Drag::Scrub)
                } else if let Some(&(row, key, outgoing, _)) = handle {
                    Some(Drag::Tangent { row, key, outgoing })
                } else if let Some(hit) = nearest_key(p) {
                    if !self.selected.contains(&hit) {
                        if !shift {
                            self.selected.clear();
                        }
                        self.selected.push(hit);
                    }
                    let original = self
                        .selected
                        .iter()
                        .filter_map(|&(r, k)| rows.get(r).and_then(|row| row.channel.keys.get(k)).map(|key| (r, k, key.t, key.v)))
                        .collect();
                    Some(Drag::Keys { start: p, original })
                } else {
                    Some(Drag::Box { start: p })
                };
            }
        }
        if let (Some(drag), Some(p)) = (self.drag.clone(), pointer) {
            match drag {
                Drag::Scrub => result.scrub = Some(snap(to_t(p.x))),
                Drag::Tangent { row, key, outgoing } => {
                    if let Some(channel) = rows.get_mut(row).map(|r| &mut r.channel) {
                        if let Some(k) = channel.keys.get_mut(key) {
                            let (kt, kv) = (to_x(k.t), to_y(k.v));
                            let d = p - egui::pos2(kt, kv);
                            let d = if outgoing { d } else { -d };
                            if d.x > 1.0 {
                                let slope = (-d.y / (rect.height() - ruler.height()) * (v1 - v0)) / (d.x / rect.width() * (t1 - t0));
                                // Tangents move together unless Shift breaks them.
                                if outgoing || !shift {
                                    k.out_slope = slope;
                                }
                                if !outgoing || !shift {
                                    k.in_slope = slope;
                                }
                                result.changed = true;
                            }
                        }
                    }
                }
                Drag::Keys { start, original } => {
                    let dt = (p.x - start.x) / rect.width() * (t1 - t0);
                    let dv = -(p.y - start.y) / (rect.height() - ruler.height()) * (v1 - v0);
                    for &(r, k, t, v) in &original {
                        if let Some(key) = rows.get_mut(r).and_then(|row| row.channel.keys.get_mut(k)) {
                            key.t = snap(t + dt);
                            key.v = v + dv;
                        }
                    }
                    result.changed = true;
                }
                Drag::Box { start } => {
                    let area = egui::Rect::from_two_pos(start, p);
                    painter.rect_stroke(area, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 190, 255)), egui::StrokeKind::Inside);
                }
            }
        }
        if response.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Box { start }) => {
                    if let Some(p) = pointer {
                        let area = egui::Rect::from_two_pos(start, p);
                        if !shift {
                            self.selected.clear();
                        }
                        for (r, k, kp) in &key_points {
                            if area.contains(*kp) && !self.selected.contains(&(*r, *k)) {
                                self.selected.push((*r, *k));
                            }
                        }
                    }
                }
                Some(Drag::Keys { .. }) => {
                    // Keep channels sorted after moving keys; selection is remapped by value.
                    let moved: Vec<(usize, Key)> = self
                        .selected
                        .iter()
                        .filter_map(|&(r, k)| rows.get(r).and_then(|row| row.channel.keys.get(k)).map(|key| (r, *key)))
                        .collect();
                    for row in rows.iter_mut() {
                        row.channel.sort();
                        row.channel.keys.dedup_by(|a, b| (a.t - b.t).abs() < 1e-4);
                    }
                    self.selected = moved
                        .into_iter()
                        .filter_map(|(r, key)| rows.get(r).and_then(|row| row.channel.keys.iter().position(|k| *k == key)).map(|k| (r, k)))
                        .collect();
                }
                _ => {}
            }
        }
        if response.clicked() {
            if let Some(p) = pointer {
                if ruler.contains(p) {
                    result.scrub = Some(snap(to_t(p.x)));
                } else {
                    match nearest_key(p) {
                        Some(hit) if shift => {
                            if let Some(i) = self.selected.iter().position(|s| *s == hit) {
                                self.selected.remove(i);
                            } else {
                                self.selected.push(hit);
                            }
                        }
                        Some(hit) => self.selected = vec![hit],
                        None if !shift => self.selected.clear(),
                        None => {}
                    }
                }
            }
        }
        // Double-click adds a key to the nearest visible curve at that time.
        if response.double_clicked() {
            if let Some(p) = pointer.filter(|p| !ruler.contains(*p)) {
                let t = snap(to_t(p.x));
                let nearest = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !self.hidden.contains(&r.label) && !r.channel.keys.is_empty())
                    .map(|(i, r)| (i, (to_y(r.channel.eval(t)) - p.y).abs()))
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                if let Some((r, _)) = nearest {
                    let value = rows[r].channel.eval(t);
                    let k = rows[r].channel.set_key(t, value);
                    self.selected = vec![(r, k)];
                    result.changed = true;
                }
            }
        }
        // Right-click: interpolation and delete for the selection.
        response.context_menu(|ui| {
            for (interp, label) in Interp::ALL {
                if ui.button(label).clicked() {
                    self.set_interp(rows, interp);
                    result.changed = true;
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Delete keys").clicked() {
                self.delete_selected(rows);
                result.changed = true;
                ui.close();
            }
        });
    }
}

fn short_label(label: &str) -> &str {
    label.rsplit(" · ").next().unwrap_or(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(interp: Interp) -> Channel {
        let mut c = Channel::default();
        for (t, v) in [(0.0, 0.0), (1.0, 10.0), (2.0, 0.0)] {
            c.set_key(t, v);
        }
        c.keys.iter_mut().for_each(|k| k.interp = interp);
        c
    }

    #[test]
    fn interpolation_modes() {
        assert_eq!(channel(Interp::Constant).eval(0.9), 0.0);
        assert_eq!(channel(Interp::Linear).eval(0.5), 5.0);
        let smooth = channel(Interp::Auto);
        // Ease in/out and no overshoot past the peak.
        assert!(smooth.eval(0.25) < 2.5);
        assert!((smooth.eval(1.0) - 10.0).abs() < 1e-5);
        assert!((0..=200).all(|i| smooth.eval(i as f32 / 100.0) <= 10.0 + 1e-4));
        // Held before the first and after the last key.
        assert_eq!(smooth.eval(-3.0), 0.0);
        assert_eq!(smooth.eval(9.0), 0.0);
    }

    #[test]
    fn bezier_tangents_shape_the_curve() {
        let mut c = channel(Interp::Bezier);
        c.keys[0].out_slope = 40.0;
        assert!(c.eval(0.1) > channel(Interp::Auto).eval(0.1));
    }

    #[test]
    fn set_key_replaces_and_inserts_in_order() {
        let mut c = Channel::constant(1.0, 5.0);
        c.set_key(0.5, 2.0);
        c.set_key(1.0, 7.0);
        assert_eq!(c.keys.iter().map(|k| (k.t, k.v)).collect::<Vec<_>>(), vec![(0.5, 2.0), (1.0, 7.0)]);
    }

    #[test]
    fn channels_round_trip_as_text() {
        let mut c = channel(Interp::Bezier);
        c.keys[1].in_slope = -3.5;
        assert_eq!(Channel::from_text(&c.to_text()), c);
    }
}
