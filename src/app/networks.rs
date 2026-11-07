// Networks: moving between them, the node editor, adding nodes, actions, undo and redo, shortcuts, cooking for display.

use crate::*;

impl ProceduralFxStudioApp {
    // Geometry node being edited, if it still exists.
    pub(crate) fn network(&self) -> Option<NodeId> {
        self.current_network.filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.kind.is_container()))
    }

    // The network containing the open one (a SOP Solver's geometry), or None at the top.
    pub(crate) fn parent_network(&self) -> Option<NodeId> {
        self.network()
            .and_then(|id| self.graph.nodes.get(&id))
            .and_then(|n| n.parent)
            .filter(|p| self.graph.nodes.get(p).is_some_and(|n| n.kind.is_container()))
    }

    // Parent assigned to nodes created in the visible network.
    pub(crate) fn view_parent(&self) -> Option<NodeId> {
        self.network().filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.context == self.context))
    }

    // Nodes cooked into the viewport: the open geometry's display, else every top-level geometry.
    pub(crate) fn view_displays(&self) -> Vec<NodeId> {
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

    // The other objects shown while inside a geometry network: every other one that isn't
    // bypassed (at the top level, all of them show).
    pub(crate) fn context_displays(&self) -> Vec<NodeId> {
        let Some(inside) = self.network().filter(|id| matches!(self.graph.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Geometry { .. }))) else {
            return Vec::new();
        };
        if self.context != AppContext::Obj {
            return Vec::new();
        }
        self.graph.geometry_ids().into_iter().filter(|id| *id != inside && self.graph.nodes.get(id).is_some_and(|n| !n.bypass)).collect()
    }

    // Opens `network` (None: the top level). The editor takes the context its nodes
    // live in: OBJ for geometry networks, MAT for a Material Builder; going back to
    // the top stays in the context of the network being left.
    pub(crate) fn enter_network(&mut self, network: Option<NodeId>) {
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

    pub(crate) fn templates_for_view(&self) -> Vec<(&'static str, Vec<NodeKind>)> {
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

    // The cached results still good for the scene as it is: a cook starts from these.
    pub(crate) fn valid_cache(&self, fingerprints: &HashMap<NodeId, u64>) -> HashMap<NodeId, Mesh> {
        self.static_cache.iter().filter(|(id, (f, _))| fingerprints.get(id) == Some(f)).map(|(id, (_, m))| (*id, m.clone())).collect()
    }

    pub(crate) fn rebuild_graph(&mut self) {
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
            self.frame_parts.clear();
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
            self.shown_parts = self.frame_parts.get(&frame_key).cloned().unwrap_or_default();
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
                    let parts = graph.cook_display_parts(&worker_displays, time, &mut worker_cache);
                    let _ = tx.send(parts);
                });
                self.simulation_rx = Some(rx);
                self.simulation_cancel = Some(cancel);
                self.simulation_key = frame_key;
                self.cancelled_simulation_key.clear();
                if !self.status.ends_with("… press Escape to cancel") {
                    self.status_before_cook = Some(std::mem::take(&mut self.status));
                }
                self.status = format!("{}… press Escape to cancel", self.graph.cooking_label(&displays));
            } else if let Some(rx) = &self.simulation_rx {
                match rx.try_recv() {
                    Ok(parts) => {
                        let mesh = crate::graph::merge_parts(parts.clone());
                        self.shown_parts = if parts.len() > 1 { parts } else { Vec::new() };
                        self.current_mesh = mesh.clone();
                        self.mesh_revision += 1;
                        const MAX_FRAME_CACHE: usize = 120;
                        if self.frame_cache.len() >= MAX_FRAME_CACHE {
                            if let Some(oldest) = self.frame_cache_order.pop_front() {
                                self.frame_cache.remove(&oldest);
                                self.frame_parts.remove(&oldest);
                            }
                        }
                        if !self.shown_parts.is_empty() {
                            self.frame_parts.insert(frame_key.clone(), self.shown_parts.clone());
                        }
                        self.frame_cache.insert(frame_key.clone(), mesh);
                        self.frame_cache_order.push_back(frame_key);
                        self.simulation_rx = None;
                        self.simulation_cancel = None;
                        self.simulation_key.clear();
                        // The progress message gives way to what it replaced; a message set
                        // meanwhile stays.
                        let before = self.status_before_cook.take().unwrap_or_default();
                        if self.status.ends_with("… press Escape to cancel") {
                            self.status = before;
                        }
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
            let parts = self.graph.cook_display_parts(&displays, self.time, &mut cache);
            self.current_mesh = crate::graph::merge_parts(parts.clone());
            self.shown_parts = if parts.len() > 1 { parts } else { Vec::new() };
            self.mesh_revision += 1;
            const MAX_FRAME_CACHE: usize = 120;
            if self.frame_cache.len() >= MAX_FRAME_CACHE {
                if let Some(oldest) = self.frame_cache_order.pop_front() {
                    self.frame_cache.remove(&oldest);
                    self.frame_parts.remove(&oldest);
                }
            }
            if !self.shown_parts.is_empty() {
                self.frame_parts.insert(frame_key.clone(), self.shown_parts.clone());
            }
            self.frame_cache.insert(frame_key.clone(), self.current_mesh.clone());
            self.frame_cache_order.push_back(frame_key);
        }
        self.template_mesh = match self.template {
            Some(id) if self.graph.nodes.contains_key(&id) => self.graph.cook(id, self.time, &mut cache),
            _ => Mesh::default(),
        };
        // Inside one object, the others still show around it, as at the top.
        let others = self.context_displays();
        let had_context = !self.context_parts.is_empty();
        self.context_parts = if others.is_empty() { Vec::new() } else { self.graph.cook_display_parts(&others, self.time, &mut cache) };
        if had_context || !self.context_parts.is_empty() {
            self.mesh_revision += 1;
        }
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

    pub(crate) fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.last_state.clone());
            self.load_state(prev);
        }
    }

    pub(crate) fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.last_state.clone());
            self.load_state(next);
        }
    }

    // A new SOP Solver starts like Houdini's: Prev_Frame and Input_1 nodes, displaying Prev_Frame.
    pub(crate) fn populate_new_node(&mut self, id: NodeId) {
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
    pub(crate) fn selected_nodes(&self) -> Vec<NodeId> {
        let Some(current) = self.selected else { return Vec::new() };
        if self.node_selection.contains(&current) {
            self.node_selection.iter().copied().filter(|id| self.graph.nodes.contains_key(id)).collect()
        } else {
            vec![current]
        }
    }

    pub(crate) fn apply(&mut self, a: Action) {
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
                // Look through the camera (it stays as it is); again, back to the free view.
                // Driving it with the view is Parameters' "Use as viewport camera".
                Some(NodeKind::Camera { .. }) => {
                    self.selected = Some(id);
                    let already = self.looking_through && self.active_camera == Some(id);
                    self.look_through(if already { None } else { Some(id) });
                    if already {
                        self.status = "Back to the free view".into();
                    }
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

    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
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

    // A Subnet's promoted parameters (adding and removing them) and its asset: saving it
    // to the library, and updating to the library's newer version.
    pub(crate) fn subnet_ui(&mut self, ui: &mut egui::Ui, id: NodeId) -> bool {
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

    // ---------------- Network editor ----------------
    pub(crate) fn node_editor(&mut self, ui: &mut egui::Ui) {
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
        self.network_canvas = canvas.rect;
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
            painter.rect(area, 0.0, egui::Color32::from_rgba_unmultiplied(255, 200, 60, 20), egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 200, 60)), egui::StrokeKind::Inside);
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
                ui.close_kind(egui::UiKind::Menu);
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
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("Rename (F2)").clicked() {
                    self.selected = Some(id);
                    self.focus_rename = true;
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("Node Info (hold middle button)").clicked() {
                    self.inspect_node = Some(id);
                    ui.close_kind(egui::UiKind::Menu);
                }
                let display_label = if is_object { "Toggle Active / Activate Camera" } else { "Set Display Flag" };
                if ui.button(display_label).clicked() {
                    actions.push(Action::Display(id));
                    ui.close_kind(egui::UiKind::Menu);
                }
                if !is_object && ui.button("Toggle Bypass (B)").clicked() {
                    actions.push(Action::Bypass(id));
                    ui.close_kind(egui::UiKind::Menu);
                }
                let together = chosen.len() > 1 && chosen.contains(&id);
                if ui.button(if together { "Duplicate selected (Ctrl+D)" } else { "Duplicate (Ctrl+D)" }).clicked() {
                    actions.push(if together { Action::DuplicateMany(chosen.iter().copied().collect()) } else { Action::Duplicate(id) });
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("Delete").clicked() {
                    actions.push(Action::Delete(id));
                    ui.close_kind(egui::UiKind::Menu);
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
                    if sel { 2.0 } else { 1.0_f32 },
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
                    egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(230, 80, 200)),
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
}
