// Picking points, primitives and edges in the viewport: box selection, and double-clicking
// an edge (or its loop).

use crate::*;

impl ProceduralFxStudioApp {
    // Box selection and edge double-clicks, given where each point is on screen (and its
    // depth). Returns an edge whose loop to select, and whether a click missed everything
    // (so the selection clears).
    pub(super) fn pick_components(&mut self, ctx: &egui::Context, response: &egui::Response, projected: &[Option<(egui::Pos2, f32)>], diagonals: &HashSet<(usize, usize)>, gizmo_clicked: bool) -> (Option<(usize, usize)>, bool) {
        let mesh = &self.current_mesh;
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
        (pending_edge_loop, clear_selection_on_miss)
    }
}
