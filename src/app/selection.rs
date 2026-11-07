// Selecting points, edges and primitives in the viewport, and making groups from them.

use crate::*;

impl ProceduralFxStudioApp {
    pub(crate) fn create_blast_from_selection(&mut self) -> bool {
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

    // Shows the node's input and selects its current group, ready for picking.
    pub(crate) fn start_group_pick(&mut self, id: NodeId, target: PickTarget) {
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
        self.shown_parts.clear();
        self.mesh_revision += 1;
        self.group_pick = Some(GroupPick { node: id, target, domain, input, previous_mode });
        self.status = "Select in the viewport, then press Enter (or ✓) to set the group; Esc cancels".into();
    }

    // Switches a running pick between points and primitives.
    pub(crate) fn set_group_pick_domain(&mut self, domain: groups::GroupDomain) {
        if let Some(pick) = self.group_pick.as_mut().filter(|p| !matches!(p.target, PickTarget::MaterialSlot(_))) {
            pick.domain = domain;
            self.selection_mode = if domain == groups::GroupDomain::Points { SelectionMode::Vertex } else { SelectionMode::Triangle };
            self.clear_component_selection();
        }
    }

    pub(crate) fn finish_group_pick(&mut self, accept: bool) {
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

    pub(crate) fn clear_component_selection(&mut self) {
        self.selected_vertices.clear();
        self.selected_triangles.clear();
        self.selected_edges.clear();
    }

    pub(crate) fn select_edge_loop(&mut self, seed: (usize, usize)) {
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

    pub(crate) fn select_all_components(&mut self) {
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

    pub(crate) fn invert_component_selection(&mut self) {
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

    pub(crate) fn grow_component_selection(&mut self) {
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

    pub(crate) fn shrink_component_selection(&mut self) {
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

    pub(crate) fn select_boundary_edges(&mut self) {
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

    pub(crate) fn select_non_manifold_edges(&mut self) {
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

    pub(crate) fn select_isolated_vertices(&mut self) {
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

    pub(crate) fn select_connected_components(&mut self) {
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
}
