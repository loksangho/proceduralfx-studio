// Graph: moved out of main.rs.

use crate::*;

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    pub(crate) pos: egui::Pos2,
    pub(crate) bypass: bool,
    pub(crate) context: AppContext,
    // Geometry node that contains this node; None for top-level (OBJ/MAT/OUT) nodes.
    pub(crate) parent: Option<NodeId>,
    // Object-level transform of Geometry objects (scale, then rotate, then translate).
    pub(crate) xform: ObjectXform,
    // The group the node applies to (Houdini's Group parameter); empty = everything.
    pub(crate) group: groups::NodeGroup,
    // Values of the ch("...") parameters its code uses (wrangles, VOP networks).
    pub(crate) channels: vex::Channels,
    // A name the user gave the node; empty shows the generated one (box3, noise7…).
    pub(crate) name: String,
}

impl Node {
    // The kind of node, for showing next to a given name ("Box", "For-Each Point"…).
    pub(crate) fn type_label(&self) -> String {
        let name = self.kind.menu_name();
        name.replace('_', " ").split(' ').map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        }).collect::<Vec<_>>().join(" ")
    }

    // What the node is called on screen: its given name, else the generated label.
    pub(crate) fn title(&self, id: NodeId) -> String {
        if self.name.trim().is_empty() { self.kind.label(id) } else { self.name.trim().to_owned() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ObjectXform {
    pub(crate) t: [f32; 3],
    pub(crate) r: [f32; 3],
    pub(crate) s: [f32; 3],
}

impl Default for ObjectXform {
    fn default() -> Self {
        Self { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] }
    }
}

impl ObjectXform {
    pub(crate) fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn apply(&self, mesh: Mesh) -> Mesh {
        if self.is_identity() {
            return mesh;
        }
        let mut out = node_transform(mesh, self.t, self.r, self.s);
        // Volumes keep their grid description: the origin moves with the object and the
        // voxel size follows the average scale (grids cannot represent rotation).
        if volume::is_volume(&out) {
            let scale = (self.s[0].abs() + self.s[1].abs() + self.s[2].abs()) / 3.0;
            if let Some(origin) = out.point_attributes.get_mut("detail:volume_origin").filter(|o| o.len() >= 3) {
                let moved = Vec3::new(origin[0], origin[1], origin[2]).mul(Vec3::from_arr(self.s)).rotate_deg(self.r).add(Vec3::from_arr(self.t));
                origin[..3].copy_from_slice(&[moved.x, moved.y, moved.z]);
            }
            for key in ["detail:volume_voxelsize", "pscale"] {
                if let Some(values) = out.point_attributes.get_mut(key) {
                    values.iter_mut().for_each(|v| *v *= scale);
                }
            }
        }
        out
    }
}

#[derive(Clone)]
pub(crate) struct Link {
    pub(crate) from: NodeId,
    pub(crate) to: NodeId,
    pub(crate) slot: usize,
    // Which of `from`'s outputs (most nodes have one; Material Fracture and RBD Bullet three).
    pub(crate) out: usize,
}

// Cache key for a node's extra outputs (output 0 is the node id itself).
pub(crate) fn out_key(id: NodeId, out: usize) -> NodeId {
    if out == 0 { id } else { id ^ (out << 56) }
}

// An animated parameter: a numeric parameter (index into NodeKind::params) or a
// component of an object's transform (tx ty tz rx ry rz sx sy sz).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ChannelTarget {
    Param(usize),
    Xform(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ChannelId {
    pub(crate) node: NodeId,
    pub(crate) target: ChannelTarget,
}

pub(crate) const XFORM_NAMES: [&str; 9] = ["tx", "ty", "tz", "rx", "ry", "rz", "sx", "sy", "sz"];

pub(crate) fn xform_component(x: &ObjectXform, i: usize) -> f32 {
    [x.t, x.r, x.s].concat()[i]
}

pub(crate) fn set_xform_component(x: &mut ObjectXform, i: usize, v: f32) {
    match i {
        0..=2 => x.t[i] = v,
        3..=5 => x.r[i - 3] = v,
        _ => x.s[i - 6] = v,
    }
}

#[derive(Default, Clone)]
pub(crate) struct Graph {
    pub(crate) nodes: HashMap<NodeId, Node>,
    pub(crate) links: Vec<Link>,
    pub(crate) next_id: NodeId,
    // Display node of each Geometry container.
    pub(crate) displays: HashMap<NodeId, NodeId>,
    // Keyframed parameters.
    pub(crate) channels: std::collections::BTreeMap<ChannelId, anim::Channel>,
    // Values written by the last apply_animation, to tell user edits from playback.
    pub(crate) applied: HashMap<ChannelId, f32>,
    // Edits to animated parameters not keyed yet (Auto Key off), as in Houdini: the value
    // holds at the frame it was made on, until it's keyed (K) or the frame changes.
    pub(crate) pending: HashMap<ChannelId, (f32, f32)>,
}

impl Graph {
    pub(crate) fn add(&mut self, kind: NodeKind, pos: egui::Pos2) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, Node { kind, pos, bypass: false, context: AppContext::Obj, parent: None, xform: ObjectXform::default(), group: groups::NodeGroup::default(), channels: vex::Channels::new(), name: String::new() });
        id
    }

    // What a background cook of `displays` is working on, for the status bar:
    // the solvers upstream of them ("Simulating Vellum + Pyro"), else an animated frame.
    pub(crate) fn cooking_label(&self, displays: &[NodeId]) -> String {
        let mut names: Vec<&'static str> = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<NodeId> = displays.to_vec();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(node) = self.nodes.get(&id) else { continue };
            if !node.bypass {
                let name = match node.kind {
                    NodeKind::Flip { .. } => Some("FLIP"),
                    NodeKind::WhitewaterSolver(_) => Some("Whitewater"),
                    NodeKind::CrowdSolver(_) => Some("Crowd"),
                    NodeKind::RbdBullet(_) => Some("RBD Bullet"),
                    NodeKind::MuscleSolver(_) => Some("Muscle"),
                    NodeKind::Pyro(_) => Some("Pyro"),
                    NodeKind::RbdSolver(_) => Some("RBD"),
                    NodeKind::VellumSolver(_) => Some("Cloth"),
                    NodeKind::SopSolver { .. } => Some("SOP Solver"),
                    NodeKind::PopNet { .. } => Some("POP Network"),
                    NodeKind::Particles { .. } => Some("Particles"),
                    _ => None,
                };
                if let Some(name) = name.filter(|n| !names.contains(n)) {
                    names.push(name);
                }
            }
            // Containers (Geometry, SOP Solver) cook their inner display node.
            if node.kind.is_container() {
                stack.extend(self.display_of(id));
            }
            stack.extend(self.links.iter().filter(|l| l.to == id).map(|l| l.from));
        }
        if names.is_empty() {
            "Cooking the animated frame".to_owned()
        } else {
            names.reverse();
            format!("Simulating {}", names.join(" + "))
        }
    }

    pub(crate) fn display_of(&self, container: NodeId) -> Option<NodeId> {
        self.displays.get(&container).copied().filter(|d| self.nodes.contains_key(d))
    }

    // Network whose display flag a node can hold: its container, or the STAGE network for LOPs.
    pub(crate) fn display_network(&self, id: NodeId) -> Option<NodeId> {
        let node = self.nodes.get(&id)?;
        node.parent.or((node.context == AppContext::Stage).then_some(STAGE_NETWORK))
    }

    pub(crate) fn is_display(&self, id: NodeId) -> bool {
        self.display_network(id).is_some_and(|p| self.displays.get(&p) == Some(&id))
    }

    // Returns false for nodes outside a container or the STAGE network.
    pub(crate) fn set_display(&mut self, id: NodeId) -> bool {
        match self.display_network(id) {
            Some(network) => {
                self.displays.insert(network, id);
                true
            }
            None => false,
        }
    }

    pub(crate) fn children(&self, container: NodeId) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent == Some(container))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // Top-level Geometry nodes in id order.
    pub(crate) fn geometry_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent.is_none() && matches!(n.kind, NodeKind::Geometry { .. }))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    pub(crate) fn node_channels(&self, id: NodeId) -> impl Iterator<Item = (&ChannelId, &anim::Channel)> {
        let lo = ChannelId { node: id, target: ChannelTarget::Param(0) };
        let hi = ChannelId { node: id, target: ChannelTarget::Xform(usize::MAX) };
        self.channels.range(lo..=hi)
    }

    pub(crate) fn is_animated(&self, id: NodeId) -> bool {
        self.node_channels(id).next().is_some()
    }

    // The node's parameters with its channels evaluated at `time`.
    pub(crate) fn kind_at(&self, id: NodeId, time: f32) -> std::borrow::Cow<'_, NodeKind> {
        let Some(node) = self.nodes.get(&id) else {
            return std::borrow::Cow::Owned(NodeKind::Null);
        };
        let mut params = node.kind.params();
        let mut any = false;
        for (channel_id, channel) in self.node_channels(id) {
            if let ChannelTarget::Param(i) = channel_id.target {
                if let Some(v) = self.pending_at(*channel_id, time).or_else(|| channel.value(time)).filter(|_| i < params.len()) {
                    params[i] = v;
                    any = true;
                }
            }
        }
        // Parameters promoted to a Subnet it's inside take the Subnet's values (at this time).
        for subnet in self.subnets_around(id) {
            let NodeKind::Subnet { spec, .. } = &self.nodes[&subnet].kind else { continue };
            if !spec.params.iter().any(|p| p.node == id) {
                continue;
            }
            let values = self.kind_at(subnet, time).params();
            for (p, value) in spec.params.iter().zip(values) {
                if p.node == id && p.index < params.len() {
                    params[p.index] = value;
                    any = true;
                }
            }
        }
        if !any {
            return std::borrow::Cow::Borrowed(&node.kind);
        }
        match NodeKind::from_params(node.kind.name(), &params) {
            Some(mut kind) => {
                kind.set_texts(&node.kind.texts());
                std::borrow::Cow::Owned(kind)
            }
            None => std::borrow::Cow::Borrowed(&node.kind),
        }
    }

    // The Subnets a node is inside, innermost first.
    pub(crate) fn subnets_around(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut at = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(parent) = at {
            let Some(node) = self.nodes.get(&parent) else { break };
            if matches!(node.kind, NodeKind::Subnet { .. }) {
                out.push(parent);
            }
            // A loop in the parents would be a broken file: stop rather than spin.
            if out.len() > 64 {
                break;
            }
            at = node.parent;
        }
        out
    }

    pub(crate) fn xform_at(&self, id: NodeId, time: f32) -> ObjectXform {
        let Some(node) = self.nodes.get(&id) else { return ObjectXform::default() };
        let mut x = node.xform;
        for (channel_id, channel) in self.node_channels(id) {
            if let (ChannelTarget::Xform(i), false) = (channel_id.target, channel.is_empty()) {
                if i < 9 {
                    if let Some(v) = self.pending_at(*channel_id, time).or_else(|| channel.value(time)) {
                        set_xform_component(&mut x, i, v);
                    }
                }
            }
        }
        x
    }

    // An unkeyed edit to this channel, if it was made at `time`.
    pub(crate) fn pending_at(&self, id: ChannelId, time: f32) -> Option<f32> {
        self.pending.get(&id).filter(|(at, _)| (at - time).abs() < 1e-6).map(|(_, v)| *v)
    }

    pub(crate) fn channel_value(&self, id: ChannelId) -> Option<f32> {
        let node = self.nodes.get(&id.node)?;
        match id.target {
            ChannelTarget::Param(i) => node.kind.params().get(i).copied(),
            ChannelTarget::Xform(i) => (i < 9).then(|| xform_component(&node.xform, i)),
        }
    }

    // Writes every channel's value at `time` into its node, so panels, lights, cameras and
    // gizmos show the animated values.
    pub(crate) fn apply_animation(&mut self, time: f32) {
        self.applied.clear();
        // Moving to another frame drops unkeyed edits, as in Houdini.
        self.pending.retain(|_, (at, _)| (*at - time).abs() < 1e-6);
        let animated: std::collections::BTreeSet<NodeId> = self.channels.keys().map(|c| c.node).collect();
        for id in animated {
            if !self.nodes.contains_key(&id) {
                continue;
            }
            let kind = self.kind_at(id, time).into_owned();
            let xform = self.xform_at(id, time);
            let node = self.nodes.get_mut(&id).expect("checked");
            node.kind = kind;
            node.xform = xform;
            let ids: Vec<ChannelId> = self.node_channels(id).map(|(c, _)| *c).collect();
            for channel_id in ids {
                if let Some(v) = self.channel_value(channel_id) {
                    self.applied.insert(channel_id, v);
                }
            }
        }
    }

    // Every animated parameter changed since apply_animation (panel edits, gizmo drags):
    // keyed at `time` with Auto Key, otherwise held as a pending edit. Returns whether
    // anything changed.
    pub(crate) fn commit_animation_edits(&mut self, time: f32, auto_key: bool) -> bool {
        let mut changed = false;
        let applied: Vec<(ChannelId, f32)> = self.applied.iter().map(|(c, v)| (*c, *v)).collect();
        for (channel_id, before) in applied {
            // Expressions drive their parameter: edits don't key over them.
            if self.channels.get(&channel_id).is_some_and(|c| c.expression.is_some()) {
                continue;
            }
            let Some(now) = self.channel_value(channel_id) else { continue };
            if (now - before).abs() > 1e-4 * before.abs().max(1.0) {
                if !auto_key {
                    self.pending.insert(channel_id, (time, now));
                    self.applied.insert(channel_id, now);
                    changed = true;
                } else if let Some(channel) = self.channels.get_mut(&channel_id) {
                    channel.set_key(time, now);
                    self.pending.remove(&channel_id);
                    self.applied.insert(channel_id, now);
                    changed = true;
                }
            }
        }
        changed
    }

    // Sets a key at `time` with the parameter's current value, creating the channel if needed.
    pub(crate) fn set_key(&mut self, id: ChannelId, time: f32) {
        if let Some(value) = self.channel_value(id) {
            self.channels.entry(id).or_default().set_key(time, value);
            self.applied.insert(id, value);
            self.pending.remove(&id);
        }
    }

    // Fingerprint of everything that affects the scene except node positions (and `skip`,
    // the active camera, which follows the viewport while orbiting).
    pub(crate) fn content_signature(&self, skip: Option<NodeId>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut ids: Vec<&NodeId> = self.nodes.keys().filter(|id| Some(**id) != skip).collect();
        ids.sort_unstable();
        for id in ids {
            let node = &self.nodes[id];
            id.hash(&mut hasher);
            // Animated parameters are compared at time 0, so playback itself is not an edit.
            format!("{:?}{:?}{:?}{}", self.kind_at(*id, 0.0), node.parent, self.xform_at(*id, 0.0), node.name).hash(&mut hasher);
            (node.bypass, node.context as u8).hash(&mut hasher);
        }
        for link in &self.links {
            (link.from, link.to, link.slot).hash(&mut hasher);
        }
        let mut displays: Vec<_> = self.displays.iter().collect();
        displays.sort_unstable();
        displays.hash(&mut hasher);
        for (channel_id, channel) in &self.channels {
            (channel_id, channel.to_text()).hash(&mut hasher);
        }
        hasher.finish()
    }

    // The Geometry object whose network (possibly through SOP Solvers) contains `id`.
    // Every node fed (through links) by `id`, and `id` itself.
    pub(crate) fn downstream(&self, id: NodeId) -> HashSet<NodeId> {
        let mut seen = HashSet::from([id]);
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            for link in self.links.iter().filter(|l| l.from == at) {
                if seen.insert(link.to) {
                    stack.push(link.to);
                }
            }
        }
        seen
    }

    // A node's path inside its object, e.g. "foreach_end1 / box2".
    pub(crate) fn path_in_object(&self, id: NodeId) -> String {
        let mut parts = Vec::new();
        let mut at = Some(id);
        while let Some(n) = at.and_then(|a| self.nodes.get(&a).map(|node| (a, node))) {
            if matches!(n.1.kind, NodeKind::Geometry { .. }) {
                break;
            }
            parts.push(n.1.title(n.0));
            at = n.1.parent;
        }
        parts.reverse();
        parts.join(" / ")
    }

    pub(crate) fn enclosing_geometry(&self, id: NodeId) -> Option<NodeId> {
        let mut current = self.nodes.get(&id)?.parent;
        while let Some(parent) = current {
            let node = self.nodes.get(&parent)?;
            if matches!(node.kind, NodeKind::Geometry { .. }) {
                return Some(parent);
            }
            current = node.parent;
        }
        None
    }

    // What the viewport shows for `ids`: like cook_many, but geometry objects without colour
    // attributes take their MAT material's colour, so the viewport matches renders.
    pub(crate) fn cook_display(&self, ids: &[NodeId], time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let mut merged: Option<Mesh> = None;
        for id in ids {
            let mut mesh = self.cook(*id, time, cache);
            // Packed geometry shows as itself.
            if packed::is_packed(&mesh) {
                mesh = packed::unpack(&mesh, true);
            }
            // Level-set VDBs are shown as their surface, like Houdini; fog volumes stay soft splats.
            if volume::is_volume(&mesh) {
                if let Some(grid) = volume::from_mesh(&mesh).ok().filter(|g| g.class == volume::Class::LevelSet) {
                    mesh = volume::to_polygons(&grid, 0.0);
                }
            }
            // Inside a geometry network the display node is shown where its object is.
            if !matches!(self.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Geometry { .. })) {
                if let Some(object) = self.enclosing_geometry(*id) {
                    mesh = self.xform_at(object, time).apply(mesh);
                }
            }
            // The object's material (inside a geometry network: the enclosing object's).
            let object = if matches!(self.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Geometry { .. })) { Some(*id) } else { self.enclosing_geometry(*id) };
            let object_material = match object.and_then(|o| self.nodes.get(&o)).map(|n| &n.kind) {
                Some(NodeKind::Geometry { material }) => *material,
                _ => None,
            };
            // Material SOP assignments: each primitive shows its own material's colour.
            if let Some(ids) = mesh.point_attributes.get(PRIM_MATERIAL).filter(|v| v.len() == mesh.indices.len() && v.iter().any(|m| *m >= 0.5)).cloned() {
                let n = mesh.indices.len();
                let point_cd = mesh.point_attributes.get("Cd").filter(|c| c.len() == mesh.vertices.len() * 3).cloned();
                let mut colours = mesh.point_attributes.get("prim:Cd").filter(|c| c.len() == n * 3).cloned().unwrap_or_else(|| {
                    (0..n).flat_map(|t| match &point_cd {
                        Some(cd) => {
                            let tri = mesh.indices[t];
                            std::array::from_fn::<f32, 3, _>(|k| tri.iter().map(|v| cd[v * 3 + k]).sum::<f32>() / 3.0)
                        }
                        None => [0.8; 3],
                    }).collect()
                });
                let mut textures: Vec<Option<String>> = Vec::new();
                let mut toon = vec![0.0f32; n];
                let mut opacity: Option<Vec<f32>> = None;
                for (t, m) in ids.iter().enumerate() {
                    let material = if *m >= 0.5 { Some((*m - 1.0).round() as usize) } else { object_material };
                    if let Some(id) = material.filter(|id| self.is_toon(*id)) {
                        toon[t] = id as f32 + 1.0;
                    }
                    if let Some(o) = material.and_then(|id| self.surface_opacity(id)) {
                        opacity.get_or_insert_with(|| [1.0, 0.0].repeat(n))[t * 2..t * 2 + 2].copy_from_slice(&o);
                    }
                    let preview = material.and_then(|m| self.material_preview(m));
                    if let Some((colour, texture, uses_cd)) = preview {
                        if !(uses_cd && point_cd.is_some()) {
                            colours[t * 3..t * 3 + 3].copy_from_slice(&colour);
                        }
                        textures.push(texture);
                    } else {
                        textures.push(None);
                    }
                }
                mesh.point_attributes.remove("Cd");
                mesh.point_attributes.insert("prim:Cd".into(), colours);
                if toon.iter().any(|t| *t > 0.0) {
                    mesh.point_attributes.insert(TOON_ATTR.into(), toon);
                }
                if let Some(opacity) = opacity {
                    mesh.point_attributes.insert(OPACITY_ATTR.into(), opacity);
                }
                // One texture for the whole mesh shows (the viewport has one per object).
                textures.dedup();
                if let [Some(texture)] = textures.as_slice() {
                    if let Ok(shaded) = sops::uv_quickshade(&mesh, texture) {
                        mesh = shaded;
                    }
                }
            } else if let Some((color, texture, uses_cd)) = object_material.and_then(|m| self.material_preview(m)) {
                let coloured = mesh.point_attributes.keys().any(|k| k == "Cd" || k.ends_with(":Cd"));
                if !(coloured && uses_cd) && !mesh.indices.is_empty() {
                    mesh.point_attributes.remove("Cd");
                    mesh.point_attributes.insert("prim:Cd".into(), color.repeat(mesh.indices.len()));
                }
                if let Some(id) = object_material.filter(|id| self.is_toon(*id) && !mesh.indices.is_empty()) {
                    mesh.point_attributes.insert(TOON_ATTR.into(), vec![id as f32 + 1.0; mesh.indices.len()]);
                }
                if let Some(o) = object_material.and_then(|id| self.surface_opacity(id)).filter(|_| !mesh.indices.is_empty()) {
                    mesh.point_attributes.insert(OPACITY_ATTR.into(), o.repeat(mesh.indices.len()));
                }
                // Image textures show in the viewport like UV Quick Shade.
                if let Some(texture) = texture {
                    if let Ok(shaded) = sops::uv_quickshade(&mesh, &texture) {
                        mesh = shaded;
                    }
                }
            }
            merged = Some(match merged {
                Some(acc) => node_merge(acc, mesh),
                None => mesh,
            });
        }
        merged.unwrap_or_default()
    }

    pub(crate) fn cook_many(&self, ids: &[NodeId], time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let mut merged: Option<Mesh> = None;
        for id in ids {
            let mesh = self.cook(*id, time, cache);
            merged = Some(match merged {
                Some(acc) => node_merge(acc, mesh),
                None => mesh,
            });
        }
        merged.unwrap_or_default()
    }

    pub(crate) fn remove(&mut self, id: NodeId) {
        for child in self.children(id) {
            self.remove(child);
        }
        self.nodes.remove(&id);
        self.links.retain(|l| l.from != id && l.to != id);
        self.displays.remove(&id);
        self.displays.retain(|_, d| *d != id);
        self.channels.retain(|c, _| c.node != id);
    }

    // Copies a container's network into another container: every node inside (and inside
    // those, for networks within networks) with its channels, the wires between them and
    // the display flags. References between the copied nodes, such as a Subnet's promoted
    // parameters, follow the copies.
    pub(crate) fn copy_children(&mut self, from: NodeId, to: NodeId) -> HashMap<NodeId, NodeId> {
        let mut map: HashMap<NodeId, NodeId> = HashMap::new();
        let mut queue = vec![(from, to)];
        while let Some((source, target)) = queue.pop() {
            for child in self.children(source) {
                let Some(src) = self.nodes.get(&child).cloned() else {
                    continue;
                };
                let container = src.kind.is_container();
                let new = self.add(src.kind, src.pos);
                if let Some(node) = self.nodes.get_mut(&new) {
                    node.bypass = src.bypass;
                    node.context = src.context;
                    node.parent = Some(target);
                    node.group = src.group.clone();
                    node.xform = src.xform;
                    node.name = src.name.clone();
                }
                let copied: Vec<(ChannelTarget, anim::Channel)> = self.node_channels(child).map(|(c, ch)| (c.target, ch.clone())).collect();
                for (channel, keys) in copied {
                    self.channels.insert(ChannelId { node: new, target: channel }, keys);
                }
                map.insert(child, new);
                if container {
                    queue.push((child, new));
                }
            }
        }
        let links: Vec<Link> = self.links.clone();
        for l in links {
            if let (Some(a), Some(b)) = (map.get(&l.from), map.get(&l.to)) {
                self.links.push(Link { from: *a, to: *b, slot: l.slot, out: l.out });
            }
        }
        let mut containers: Vec<(NodeId, NodeId)> = map.iter().map(|(&a, &b)| (a, b)).collect();
        containers.push((from, to));
        for (source, target) in containers {
            if let Some(display) = self.displays.get(&source).and_then(|d| map.get(d)) {
                self.displays.insert(target, *display);
            }
        }
        let follow = |id: NodeId| map.get(&id).copied().unwrap_or(id);
        for id in map.values().copied().chain(std::iter::once(to)) {
            if let Some(node) = self.nodes.get_mut(&id) {
                remap_node_ids(&mut node.kind, follow);
            }
        }
        map
    }

    // Every node inside a container, at any depth.
    pub(crate) fn descendants(&self, container: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut queue = vec![container];
        while let Some(at) = queue.pop() {
            for child in self.children(at) {
                out.push(child);
                queue.push(child);
            }
        }
        out.sort_unstable();
        out
    }

    // A Subnet as an asset file: a scene of just it and its network, with its name,
    // version and description.
    pub(crate) fn asset_text(&self, subnet: NodeId, name: &str, version: u32, description: &str) -> String {
        let mut keep: HashSet<NodeId> = self.descendants(subnet).into_iter().collect();
        keep.insert(subnet);
        let mut g = Graph { next_id: self.next_id, ..Graph::default() };
        for &id in &keep {
            let mut node = self.nodes[&id].clone();
            if id == subnet {
                node.parent = None;
                if let NodeKind::Subnet { spec, .. } = &mut node.kind {
                    spec.asset = Some((name.to_owned(), version));
                    spec.description = description.to_owned();
                }
            }
            g.nodes.insert(id, node);
        }
        g.links = self.links.iter().filter(|l| keep.contains(&l.from) && keep.contains(&l.to)).cloned().collect();
        g.displays = self.displays.iter().filter(|(c, d)| keep.contains(c) && keep.contains(d)).map(|(&c, &d)| (c, d)).collect();
        g.channels = self.channels.iter().filter(|(c, _)| keep.contains(&c.node)).map(|(c, ch)| (*c, ch.clone())).collect();
        g.serialize() + &assets::info_line(name, version, description)
    }

    // The Subnets placed from asset `name` at a version older than `version`.
    pub(crate) fn asset_copies(&self, name: &str, version: u32) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| matches!(&n.kind, NodeKind::Subnet { spec: assets::Spec { asset: Some((a, v)), .. }, .. } if a == name && *v < version))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // Updates every older copy of asset `name` from `text` (the library's version), each
    // keeping its sliders' values. Copies inside copies go too, outer ones first.
    pub(crate) fn update_asset_copies(&mut self, name: &str, version: u32, text: &str) -> Result<usize, String> {
        let mut count = 0;
        // Each update can remove nested copies, so look again after each.
        while let Some(&id) = self.asset_copies(name, version).first() {
            self.fill_from_asset(id, text)?;
            count += 1;
            if count > 10_000 {
                break;
            }
        }
        Ok(count)
    }

    // Fills a Subnet from an asset file: its network (replacing what was inside), its
    // promoted parameters and the asset's name and version. Sliders keep their values
    // where the new version has a slider of the same name. Err says why it couldn't.
    pub(crate) fn fill_from_asset(&mut self, subnet: NodeId, text: &str) -> Result<(), String> {
        if let Some(problem) = scene_problem(text) {
            return Err(problem);
        }
        let asset = Graph::deserialize(text);
        let root = asset
            .nodes
            .iter()
            .filter(|(_, n)| n.parent.is_none() && matches!(n.kind, NodeKind::Subnet { .. }))
            .map(|(id, _)| *id)
            .min()
            .ok_or("the file has no Subnet in it")?;
        let NodeKind::Subnet { values: new_values, spec: new_spec } = asset.nodes[&root].kind.clone() else { unreachable!() };
        let Some(NodeKind::Subnet { values: old_values, spec: old_spec }) = self.nodes.get(&subnet).map(|n| n.kind.clone()) else {
            return Err("that isn't a Subnet".into());
        };
        for child in self.children(subnet) {
            self.remove(child);
        }
        let offset = self.next_id;
        let map = |id: NodeId| id + offset;
        for (&id, node) in &asset.nodes {
            if id == root {
                continue;
            }
            let mut node = node.clone();
            node.parent = node.parent.map(|p| if p == root { subnet } else { map(p) });
            remap_node_ids(&mut node.kind, map);
            self.nodes.insert(map(id), node);
        }
        let inside = |id: NodeId| if id == root { None } else { Some(map(id)) };
        self.links.extend(asset.links.iter().filter_map(|l| Some(Link { from: inside(l.from)?, to: inside(l.to)?, slot: l.slot, out: l.out })));
        for (&container, &display) in &asset.displays {
            let container = if container == root { subnet } else { map(container) };
            self.displays.insert(container, map(display));
        }
        for (c, ch) in &asset.channels {
            if c.node != root {
                self.channels.insert(ChannelId { node: map(c.node), target: c.target }, ch.clone());
            }
        }
        self.next_id = offset + asset.next_id;
        let mut spec = new_spec;
        spec.remap(map);
        let values = spec
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let kept = old_spec.params.iter().position(|o| o.label == p.label).and_then(|k| old_values.get(k));
                kept.copied().unwrap_or_else(|| new_values.get(i).copied().unwrap_or(0.0))
            })
            .collect();
        if let Some(node) = self.nodes.get_mut(&subnet) {
            node.kind = NodeKind::Subnet { values, spec };
        }
        Ok(())
    }

    pub(crate) fn duplicate(&mut self, id: NodeId) -> Option<NodeId> {
        let n = self.nodes.get(&id)?.clone();
        let is_geometry = matches!(n.kind, NodeKind::Geometry { .. });
        let new = self.add(n.kind, n.pos + egui::vec2(30.0, 60.0));
        if let Some(node) = self.nodes.get_mut(&new) {
            node.bypass = n.bypass;
            node.context = n.context;
            node.parent = n.parent;
            node.xform = n.xform;
        }
        let copied: Vec<(ChannelTarget, anim::Channel)> = self.node_channels(id).map(|(c, ch)| (c.target, ch.clone())).collect();
        for (target, channel) in copied {
            self.channels.insert(ChannelId { node: new, target }, channel);
        }
        if self.nodes.get(&new).is_some_and(|n| n.kind.is_container()) {
            self.copy_children(id, new);
            if !is_geometry {
                let inputs: Vec<Link> = self.links.iter().filter(|l| l.to == id).cloned().collect();
                for l in inputs {
                    self.links.push(Link { from: l.from, to: new, slot: l.slot, out: l.out });
                }
            }
            return Some(new);
        }
        let inputs: Vec<Link> = self.links.iter().filter(|l| l.to == id).cloned().collect();
        for l in inputs {
            self.links.push(Link { from: l.from, to: new, slot: l.slot, out: l.out });
        }
        Some(new)
    }

    // Duplicates several nodes at once: wires between them are copied between the copies,
    // and wires coming in from outside feed the copies too, as with one node. Returns
    // each original's copy.
    pub(crate) fn duplicate_many(&mut self, ids: &[NodeId]) -> HashMap<NodeId, NodeId> {
        let ids: Vec<NodeId> = ids.iter().copied().filter(|id| self.nodes.contains_key(id)).collect();
        let mut map = HashMap::new();
        for &id in &ids {
            let n = &self.nodes[&id];
            let new = self.add(n.kind.clone(), n.pos + egui::vec2(30.0, 60.0));
            map.insert(id, new);
        }
        for &id in &ids {
            let src = self.nodes[&id].clone();
            let new = map[&id];
            if let Some(node) = self.nodes.get_mut(&new) {
                node.bypass = src.bypass;
                node.context = src.context;
                node.parent = src.parent;
                node.xform = src.xform;
                node.group = src.group.clone();
                // References to other copied nodes (a loop's Begin, say) follow them.
                remap_node_ids(&mut node.kind, |other| map.get(&other).copied().unwrap_or(other));
            }
            let copied: Vec<(ChannelTarget, anim::Channel)> = self.node_channels(id).map(|(c, ch)| (c.target, ch.clone())).collect();
            for (target, channel) in copied {
                self.channels.insert(ChannelId { node: new, target }, channel);
            }
            if src.kind.is_container() {
                self.copy_children(id, new);
            }
        }
        let incoming: Vec<Link> = self.links.iter().filter(|l| map.contains_key(&l.to)).cloned().collect();
        for l in incoming {
            let outside = !map.contains_key(&l.from);
            // An object container copied on its own isn't fed by what fed the original.
            if outside && matches!(self.nodes[&l.to].kind, NodeKind::Geometry { .. }) {
                continue;
            }
            self.links.push(Link { from: map.get(&l.from).copied().unwrap_or(l.from), to: map[&l.to], slot: l.slot, out: l.out });
        }
        map
    }

    // Adds a whole other scene (such as a preset) beside this one, with fresh ids. Its
    // top-level nodes go to the right of what's here. Returns each of its nodes' new ids.
    pub(crate) fn merge(&mut self, other: &Graph) -> HashMap<NodeId, NodeId> {
        let offset = self.next_id;
        let map = |id: NodeId| id + offset;
        let top = |g: &Graph| g.nodes.values().filter(|n| n.parent.is_none()).map(|n| n.pos.x).collect::<Vec<f32>>();
        let (here, there) = (top(self), top(other));
        let shift = match (here.iter().cloned().reduce(f32::max), there.iter().cloned().reduce(f32::min)) {
            (Some(right), Some(left)) => egui::vec2(right + 220.0 - left, 0.0),
            _ => egui::Vec2::ZERO,
        };
        let mut ids = HashMap::new();
        for (&id, node) in &other.nodes {
            let mut node = node.clone();
            node.parent = node.parent.map(map);
            remap_node_ids(&mut node.kind, map);
            if node.parent.is_none() {
                node.pos += shift;
            }
            self.nodes.insert(map(id), node);
            ids.insert(id, map(id));
        }
        self.links.extend(other.links.iter().map(|l| Link { from: map(l.from), to: map(l.to), slot: l.slot, out: l.out }));
        self.displays.extend(other.displays.iter().map(|(&c, &d)| (map(c), map(d))));
        self.channels.extend(other.channels.iter().map(|(c, ch)| (ChannelId { node: map(c.node), target: c.target }, ch.clone())));
        self.next_id = offset + other.next_id;
        ids
    }

    pub(crate) fn input_of(&self, to: NodeId, slot: usize) -> Option<NodeId> {
        self.links.iter().find(|l| l.to == to && l.slot == slot).map(|l| l.from)
    }

    pub(crate) fn depends_on(&self, node: NodeId, candidate: NodeId) -> bool {
        node == candidate
            || self
                .links
                .iter()
                .filter(|l| l.to == node)
                .any(|l| self.depends_on(l.from, candidate))
    }

    pub(crate) fn connect(&mut self, from: NodeId, to: NodeId, slot: usize) -> bool {
        if self.depends_on(from, to) {
            return false;
        }
        self.connect_out(from, 0, to, slot)
    }

    // Connects output `out` of `from` to input `slot` of `to`.
    pub(crate) fn connect_out(&mut self, from: NodeId, out: usize, to: NodeId, slot: usize) -> bool {
        if self.depends_on(from, to) {
            return false;
        }
        self.links.retain(|l| !(l.to == to && l.slot == slot));
        self.links.push(Link { from, to, slot, out });
        true
    }

    // The node (and which of its outputs) connected to input `slot`.
    pub(crate) fn input_link(&self, to: NodeId, slot: usize) -> Option<(NodeId, usize)> {
        self.links.iter().find(|l| l.to == to && l.slot == slot).map(|l| (l.from, l.out))
    }

    // Output `out` of node `id`: extra outputs are cooked with output 0 and cached beside it.
    pub(crate) fn cook_output(&self, id: NodeId, out: usize, time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let first = self.cook(id, time, cache);
        if out == 0 {
            return first;
        }
        if let Some(m) = cache.get(&out_key(id, out)) {
            return m.clone();
        }
        // Bypassed: each output passes its matching input through.
        match self.input_link(id, out).filter(|_| self.nodes.get(&id).is_some_and(|n| n.bypass)) {
            Some((src, o)) => self.cook_output(src, o, time, cache),
            None => Mesh::default(),
        }
    }

    // Input `slot` cooked on its own at `time`, from whichever output it's wired to.
    pub(crate) fn cook_input_fresh(&self, id: NodeId, slot: usize, time: f32) -> Mesh {
        self.input_link(id, slot).map(|(src, out)| self.cook_output(src, out, time, &mut HashMap::new())).unwrap_or_default()
    }

    /// True if this node or anything upstream animates with time.
    // Records a node's cook error for its parameter panel; errors cook to empty geometry.
    pub(crate) fn report(&self, id: NodeId, result: Result<Mesh, String>) -> Mesh {
        node_errors(|errors| match result {
            Ok(mesh) => {
                errors.remove(&id);
                mesh
            }
            Err(error) => {
                errors.insert(id, error);
                Mesh::default()
            }
        })
    }

    // VDB from Polygons as a level set volume: OpenVDB when built with it, else the Rust rasterizer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn vdb_volume(&self, id: NodeId, source: Mesh, time: f32, voxel: f32, padding: usize, operation: usize, iterations: usize) -> Result<Mesh, String> {
        #[cfg(feature = "native-openvdb")]
        if !volume::is_volume(&source) {
            if let Some(native) = self.cook_native_volume(id, time, &mut HashMap::new()) {
                if let Some(mesh) = native_volume_voxels(&native) {
                    return Ok(mesh);
                }
            }
        }
        let _ = (id, time);
        let mut grid = if volume::is_volume(&source) {
            volume::from_mesh(&source)?
        } else {
            volume::from_polygons(&source, voxel, 3.0 + padding as f32)?
        };
        let shift = iterations as f32 * grid.voxel;
        match operation {
            1 => volume::offset(&mut grid, shift),
            2 => volume::offset(&mut grid, -shift),
            3 => volume::smooth(&mut grid, iterations),
            _ => {}
        }
        Ok(volume::to_mesh(&grid))
    }

    // The USD stage a LOP node produces from its inputs.
    pub(crate) fn cook_stage(&self, id: NodeId, time: f32, cache: &mut HashMap<NodeId, Arc<usd::Stage>>) -> Arc<usd::Stage> {
        if let Some(stage) = cache.get(&id) {
            return stage.clone();
        }
        let Some(node) = self.nodes.get(&id) else {
            return Arc::default();
        };
        let input = |slot: usize, cache: &mut HashMap<NodeId, Arc<usd::Stage>>| {
            self.input_of(id, slot)
                .filter(|src| self.nodes.get(src).is_some_and(|n| n.kind.is_lop()))
                .map(|src| self.cook_stage(src, time, cache))
                .unwrap_or_default()
        };
        let mut error = None;
        let stage = if node.bypass {
            input(0, cache)
        } else {
            let mut stage = (*input(0, cache)).clone();
            let animated = self.kind_at(id, time);
            match animated.as_ref() {
                NodeKind::LopSopImport { geometry, path } => {
                    let objects = match geometry {
                        Some(g) => vec![*g],
                        None => self.geometry_ids().into_iter().filter(|g| !self.nodes[g].bypass).collect(),
                    };
                    for object in objects {
                        let mesh = self.cook(object, time, &mut HashMap::new());
                        if mesh.vertices.is_empty() {
                            continue;
                        }
                        let prim_path = if geometry.is_some() { path.clone() } else { format!("{path}/geometry{object}") };
                        let object_material = match self.nodes.get(&object).map(|n| &n.kind) {
                            Some(NodeKind::Geometry { material }) => *material,
                            _ => None,
                        };
                        let parts = split_by_material(mesh, object_material, |m| self.nodes.get(&m).is_some_and(|n| n.kind.is_material()));
                        if parts.len() == 1 {
                            let (mesh, m) = parts.into_iter().next().unwrap();
                            let material = m.and_then(|m| self.material_prim(&mut stage, m));
                            let prim = stage.define(&prim_path, "Mesh");
                            prim.mesh = Some(Arc::new(mesh));
                            prim.binding = material;
                        } else {
                            // Several materials: one Mesh child per material under an Xform.
                            stage.define(&prim_path, "Xform");
                            for (mesh, m) in parts {
                                let material = m.and_then(|m| self.material_prim(&mut stage, m));
                                let child = format!("{prim_path}/{}", m.map_or("default".to_owned(), |m| format!("mat{m}")));
                                let prim = stage.define(&child, "Mesh");
                                prim.mesh = Some(Arc::new(mesh));
                                prim.binding = material;
                            }
                        }
                    }
                }
                NodeKind::LopReference { file, path, sublayer } => match usd::load_file(file, time) {
                    Ok(loaded) if *sublayer => stage.merge(&loaded),
                    Ok(loaded) => stage.merge(&loaded.rerooted(path)),
                    Err(e) => error = Some(e),
                },
                NodeKind::LopPrimitive { shape, path, size, t } => {
                    let shape = (*shape).min(LOP_SHAPES.len() - 1);
                    let prim = stage.define(path, LOP_SHAPES[shape]);
                    prim.xform = usd::trs(*t, [0.0; 3], [1.0; 3]);
                    let size = size.max(0.001);
                    let attrs: &[(&str, f32)] = match shape {
                        0 => &[("radius", size)],
                        1 => &[("size", size * 2.0)],
                        4 => &[("radius", size * 0.5), ("height", size)],
                        _ => &[("radius", size), ("height", size * 2.0)],
                    };
                    for (name, value) in attrs {
                        prim.attrs.insert(name.to_string(), usd::AttrValue::Float(*value));
                    }
                    if shape >= 2 {
                        prim.attrs.insert("axis".into(), usd::AttrValue::Token("Y".into()));
                    }
                }
                NodeKind::LopTransform { pattern, t, r, s } => {
                    let m = usd::trs(*t, *r, *s);
                    for (path, prim) in stage.prims.iter_mut() {
                        if usd::matches_pattern(pattern, path) {
                            prim.xform = usd::mat_mul(&prim.xform, &m);
                        }
                    }
                }
                NodeKind::LopLight { kind, path, position, target, color, intensity, size } => {
                    let kind = (*kind).min(LOP_LIGHTS.len() - 1);
                    let prim = stage.define(path, LOP_LIGHTS[kind]);
                    prim.xform = if kind == 2 { usd::IDENTITY } else { usd::look_at(*position, *target) };
                    prim.attrs.insert("intensity".into(), usd::AttrValue::Float(*intensity));
                    prim.attrs.insert("color".into(), usd::AttrValue::Vec3(*color));
                    let sizes: &[&str] = match kind {
                        0 => &["angle"],
                        1 => &["radius"],
                        3 => &["width", "height"],
                        _ => &[],
                    };
                    for name in sizes {
                        prim.attrs.insert(name.to_string(), usd::AttrValue::Float(*size));
                    }
                }
                NodeKind::LopCamera { path, position, target, focal, aperture } => {
                    let prim = stage.define(path, "Camera");
                    prim.xform = usd::look_at(*position, *target);
                    prim.attrs.insert("focalLength".into(), usd::AttrValue::Float(*focal));
                    prim.attrs.insert("horizontalAperture".into(), usd::AttrValue::Float(*aperture));
                    prim.attrs.insert("verticalAperture".into(), usd::AttrValue::Float(aperture * 9.0 / 16.0));
                }
                NodeKind::LopMaterial { pattern, material } => match material.and_then(|m| self.material_prim(&mut stage, m)) {
                    Some(material_path) => {
                        for (path, prim) in stage.prims.iter_mut() {
                            if prim.type_name != "Material" && usd::matches_pattern(pattern, path) {
                                prim.binding = Some(material_path.clone());
                            }
                        }
                    }
                    None => error = Some("choose a MAT material to assign".into()),
                },
                NodeKind::LopPrune { pattern, hide } => {
                    for (path, prim) in stage.prims.iter_mut() {
                        if usd::matches_pattern(pattern, path) {
                            if *hide {
                                prim.visible = false;
                            } else {
                                prim.active = false;
                            }
                        }
                    }
                }
                NodeKind::LopMerge => {
                    // Every further input, in order, each layered over the ones before.
                    let mut slots: Vec<usize> = self.links.iter().filter(|l| l.to == id && l.slot > 0).map(|l| l.slot).collect();
                    slots.sort_unstable();
                    slots.dedup();
                    for slot in slots {
                        let layer = input(slot, cache);
                        stage.merge(&layer);
                    }
                }
                _ => {}
            }
            Arc::new(stage)
        };
        node_errors(|errors| match error {
            Some(e) => {
                errors.insert(id, e);
            }
            None => {
                errors.remove(&id);
            }
        });
        cache.insert(id, stage.clone());
        stage
    }

    // The For-Each End paired with a Begin.
    pub(crate) fn foreach_end_of(&self, begin: NodeId) -> Option<NodeId> {
        self.nodes.iter().find(|(_, n)| matches!(&n.kind, NodeKind::ForEachEnd(s) if s.begin == Some(begin))).map(|(id, _)| *id)
    }

    // A loop's body: nodes downstream of `begin` up to (not including) `end`, and `begin` itself.
    pub(crate) fn loop_body(&self, begin: NodeId, end: NodeId) -> Vec<NodeId> {
        let mut body = vec![begin];
        let mut i = 0;
        while i < body.len() {
            let at = body[i];
            for l in self.links.iter().filter(|l| l.from == at) {
                if l.to != end && !body.contains(&l.to) {
                    body.push(l.to);
                }
            }
            i += 1;
        }
        body
    }

    // Whether `node` sits inside a Compile Block (a paired Compile Begin up its input chain).
    pub(crate) fn in_compile_block(&self, node: NodeId) -> bool {
        let mut at = Some(node);
        for _ in 0..256 {
            let Some(id) = at else { return false };
            if matches!(self.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::CompileBegin))
                && self.nodes.values().any(|n| matches!(n.kind, NodeKind::CompileEnd { begin: Some(b) } if b == id))
            {
                return true;
            }
            at = self.input_of(id, 0);
        }
        false
    }

    // Runs a For-Each loop: the body once per iteration, results merged (or fed back).
    pub(crate) fn cook_foreach(&self, end: NodeId, set: &flow::ForEachSettings, time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Result<Mesh, String> {
        let begin = set
            .begin
            .filter(|b| matches!(self.nodes.get(b).map(|n| &n.kind), Some(NodeKind::ForEachBegin)))
            .ok_or("this For-Each End has lost its For-Each Begin: add the loop again from the Flow Control menu")?;
        let body_out = self.input_of(end, 0).ok_or("connect the loop's last node to the For-Each End")?;
        let source = self.input_of(begin, 0).map(|s| self.cook(s, time, cache)).unwrap_or_default();
        let body = self.loop_body(begin, end);
        let run = |input: Mesh, cache: &mut HashMap<NodeId, Mesh>| -> Mesh {
            FOREACH_INPUT.with(|m| m.borrow_mut().insert(begin, input));
            for b in &body {
                cache.remove(b);
            }
            let out = self.cook(body_out, time, cache);
            FOREACH_INPUT.with(|m| m.borrow_mut().remove(&begin));
            out
        };
        let result = if set.mode == flow::FEEDBACK {
            let count = set.iterations.min(100_000);
            let mut current = source;
            for i in 0..count {
                if set.single_pass >= 0 && i as i32 > set.single_pass {
                    break;
                }
                current = run(flow::stamp(current, i, count, set.start + i as f32 * set.increment), cache);
            }
            current
        } else {
            let inputs: Vec<Mesh> = if set.mode == flow::NUMBER {
                let count = set.iterations.min(100_000);
                (0..count).map(|i| flow::stamp(source.clone(), i, count, set.start + i as f32 * set.increment)).collect()
            } else {
                let pieces = flow::pieces(&source, set)?;
                let n = pieces.len();
                pieces.into_iter().enumerate().map(|(i, p)| flow::stamp(p, i, n, i as f32)).collect()
            };
            let inputs: Vec<Mesh> = match usize::try_from(set.single_pass) {
                Ok(only) => inputs.into_iter().skip(only).take(1).collect(),
                Err(_) => inputs,
            };
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(inputs.len().max(1));
            let outputs: Vec<Mesh> = if set.multithread && threads > 1 && self.in_compile_block(begin) {
                // Compiled: iterations run on several threads, each with its own cook cache.
                let mut slots: Vec<Option<Mesh>> = vec![None; inputs.len()];
                std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..threads)
                        .map(|t| {
                            let (inputs, run) = (&inputs, &run);
                            scope.spawn(move || {
                                let mut local = HashMap::new();
                                (t..inputs.len()).step_by(threads).map(|i| (i, run(inputs[i].clone(), &mut local))).collect::<Vec<_>>()
                            })
                        })
                        .collect();
                    for h in handles {
                        for (i, mesh) in h.join().unwrap_or_default() {
                            slots[i] = Some(mesh);
                        }
                    }
                });
                slots.into_iter().map(Option::unwrap_or_default).collect()
            } else {
                inputs.into_iter().map(|input| run(input, cache)).collect()
            };
            outputs.into_iter().fold(Mesh::default(), node_merge)
        };
        // Body results of the last iteration mustn't be mistaken for the loop's outside.
        for b in &body {
            cache.remove(b);
        }
        Ok(result)
    }

    // Object Merge: other objects' (or nodes') geometry, in this object's space when asked.
    pub(crate) fn object_merge(&self, id: NodeId, sources: &[Option<NodeId>], into_this: bool, time: f32) -> Result<Mesh, String> {
        let own = self.enclosing_geometry(id);
        if MERGE_DEPTH.with(|d| d.get()) > 16 {
            return Err("these Object Merges merge each other in a loop".into());
        }
        MERGE_DEPTH.with(|d| d.set(d.get() + 1));
        let mut merged = Mesh::default();
        let mut problem = None;
        for src in sources.iter().flatten() {
            let Some(node) = self.nodes.get(src) else {
                problem = Some(format!("node {src} no longer exists"));
                continue;
            };
            if Some(*src) == own {
                problem = Some("an Object Merge can't merge its own object".into());
                continue;
            }
            // An object's display output in its own space, with its transform; or a SOP node and its object's.
            let (mesh, xform) = match node.kind {
                NodeKind::Geometry { .. } => (self.display_of(*src).map(|d| self.cook(d, time, &mut HashMap::new())).unwrap_or_default(), self.xform_at(*src, time)),
                _ => (self.cook(*src, time, &mut HashMap::new()), self.enclosing_geometry(*src).map_or(ObjectXform::default(), |o| self.xform_at(o, time))),
            };
            let mesh = if into_this {
                let world = xform.apply(mesh);
                match own {
                    Some(o) => inverse_xform(&self.xform_at(o, time), world),
                    None => world,
                }
            } else {
                mesh
            };
            merged = node_merge(merged, mesh);
        }
        MERGE_DEPTH.with(|d| d.set(d.get() - 1));
        match problem {
            Some(p) if merged.vertices.is_empty() => Err(p),
            _ => Ok(merged),
        }
    }

    // A TOP node's work items, generated from its input's.
    pub(crate) fn top_items(&self, id: NodeId) -> Result<Vec<tops::WorkItem>, String> {
        self.top_items_depth(id, 0)
    }

    pub(crate) fn top_items_depth(&self, id: NodeId, depth: usize) -> Result<Vec<tops::WorkItem>, String> {
        let Some(NodeKind::Top(node)) = self.nodes.get(&id).map(|n| &n.kind) else {
            return Err("not a TOP node".into());
        };
        if depth > 64 {
            return Err("the TOP network has a loop".into());
        }
        let upstream = match self.input_of(id, 0) {
            Some(src) => Some(self.top_items_depth(src, depth + 1).map_err(|e| format!("{}: {e}", self.nodes[&src].title(src)))?),
            None => None,
        };
        tops::generate(node, upstream.as_deref(), |target| match self.nodes.get(&target).map(|n| &n.kind) {
            Some(NodeKind::MoonRayRender { output, opts, .. }) => Some(tops::FetchTarget {
                render: true,
                output: output.clone(),
                frames: if opts.sequence {
                    frame_times(opts.range_start, opts.range_end, opts.range_step).iter().map(|t| t * sims::SIM_FPS + 1.0).collect()
                } else {
                    vec![1.0]
                },
                cache: None,
            }),
            Some(NodeKind::FileExport { file }) => Some(tops::FetchTarget { render: false, output: file.clone(), frames: vec![1.0], cache: None }),
            Some(NodeKind::Cache { cache_id, range, .. }) => {
                let (start, end, step) = (range[0], range[1].max(range[0]), range[2].max(0.001));
                let count = (((end - start) / step).floor() as usize + 1).min(100_000);
                Some(tops::FetchTarget { render: false, output: String::new(), frames: (0..count).map(|i| start + i as f32 * step).collect(), cache: Some(*cache_id) })
            }
            _ => None,
        })
    }

    // Sets parameter `index` of `node`; returns the old value (for wedges, put back afterwards).
    pub(crate) fn set_param(&mut self, node: NodeId, index: usize, value: f32) -> Option<f32> {
        let n = self.nodes.get_mut(&node)?;
        let mut params = n.kind.params();
        let old = *params.get(index)?;
        params[index] = value;
        let mut kind = NodeKind::from_params(n.kind.name(), &params)?;
        kind.set_texts(&n.kind.texts());
        n.kind = kind;
        Some(old)
    }

    // The Surface Output a Material Builder renders: its display node, else its first one.
    pub(crate) fn builder_output(&self, builder: NodeId) -> Option<NodeId> {
        let is_output = |id: &NodeId| matches!(self.nodes.get(id).map(|n| &n.kind), Some(NodeKind::ShaderNode(n)) if n.op == shading::SURFACE_OUTPUT);
        self.display_of(builder).filter(is_output).or_else(|| {
            let mut outputs: Vec<NodeId> = self.children(builder).into_iter().filter(is_output).collect();
            outputs.sort_unstable();
            outputs.first().copied()
        })
    }

    pub(crate) fn builder_nodes(&self, builder: NodeId) -> HashMap<NodeId, shading::ShaderNode> {
        self.children(builder)
            .into_iter()
            .filter_map(|child| match &self.nodes.get(&child)?.kind {
                NodeKind::ShaderNode(n) if !self.nodes[&child].bypass => Some((child, n.clone())),
                _ => None,
            })
            .collect()
    }

    // What the viewport shows for a MAT node: (colour, image texture, whether point Cd is used).
    // A Principled Shader that isn't fully opaque: [opacity, 1 if it uses point Alpha].
    pub(crate) fn surface_opacity(&self, material: NodeId) -> Option<[f32; 2]> {
        match &self.nodes.get(&material)?.kind {
            NodeKind::Shader(v) if v.table == shading::Table::Principled => {
                let (opacity, alpha) = (v.float("opacity").clamp(0.0, 1.0), v.flag("use_point_alpha"));
                (opacity < 1.0 || alpha).then_some([opacity, alpha as u8 as f32])
            }
            _ => None,
        }
    }

    pub(crate) fn is_toon(&self, material: NodeId) -> bool {
        matches!(self.nodes.get(&material).map(|n| &n.kind), Some(NodeKind::Shader(v)) if v.table == shading::Table::Toon)
    }

    pub(crate) fn material_preview(&self, material: NodeId) -> Option<([f32; 3], Option<String>, bool)> {
        match &self.nodes.get(&material)?.kind {
            NodeKind::Material { color, .. } => Some((*color, None, true)),
            NodeKind::Shader(v) => {
                let texture = (v.table == shading::Table::Principled && !v.texture(0).is_empty()).then(|| v.texture(0).to_owned());
                let uses_cd = matches!(v.table, shading::Table::Principled | shading::Table::Toon) && v.flag("use_point_color");
                Some((v.viewport_color(), texture, uses_cd))
            }
            NodeKind::MaterialBuilder => {
                let nodes = self.builder_nodes(material);
                let output = self.builder_output(material)?;
                let (colour, texture) = shading::builder_preview(&nodes, |to, slot| self.input_of(to, slot), output);
                Some((colour, texture, false))
            }
            _ => None,
        }
    }

    // Adds a UsdPreviewSurface Material prim for a MAT network node; returns its path.
    pub(crate) fn material_prim(&self, stage: &mut usd::Stage, material: NodeId) -> Option<String> {
        let surface = match &self.nodes.get(&material)?.kind {
            NodeKind::Shader(v) if v.table == shading::Table::Principled => Some(v.clone()),
            NodeKind::MaterialBuilder => self.builder_output(material).and_then(|o| match &self.nodes.get(&o)?.kind {
                NodeKind::ShaderNode(n) => Some(n.surface.clone()),
                _ => None,
            }),
            _ => None,
        };
        if let Some(v) = surface {
            let path = format!("/materials/mat{material}");
            let prim = stage.define(&path, "Material");
            prim.material_node = Some(material);
            let attrs = &mut prim.attrs;
            let (colour, _, _) = self.material_preview(material)?;
            attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(colour));
            attrs.insert("roughness".into(), usd::AttrValue::Float(v.float("roughness")));
            attrs.insert("metallic".into(), usd::AttrValue::Float(v.float("metallic")));
            attrs.insert("ior".into(), usd::AttrValue::Float(v.float("ior")));
            attrs.insert("clearcoat".into(), usd::AttrValue::Float(v.float("clearcoat")));
            let opacity = v.float("opacity") * (1.0 - 0.8 * v.float("transmission"));
            if opacity < 1.0 {
                attrs.insert("opacity".into(), usd::AttrValue::Float(opacity));
            }
            if v.float("emission_intensity") > 0.0 {
                attrs.insert("emissiveColor".into(), usd::AttrValue::Vec3(v.color("emission_color").map(|c| c * v.float("emission_intensity"))));
            }
            return Some(path);
        }
        if let Some((colour, _, _)) = self.material_preview(material).filter(|_| !matches!(self.nodes[&material].kind, NodeKind::Material { .. })) {
            // Toon, hair and pyro shaders reach USD as their colour.
            let path = format!("/materials/mat{material}");
            let prim = stage.define(&path, "Material");
            prim.material_node = Some(material);
            prim.attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(colour));
            return Some(path);
        }
        let NodeKind::Material { kind, color, roughness, value } = self.nodes.get(&material)?.kind else {
            return None;
        };
        let path = format!("/materials/mat{material}");
        let prim = stage.define(&path, "Material");
        prim.material_node = Some(material);
        let attrs = &mut prim.attrs;
        attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(color));
        attrs.insert("roughness".into(), usd::AttrValue::Float(roughness));
        attrs.insert("metallic".into(), usd::AttrValue::Float(if kind == 1 { 1.0 } else { 0.0 }));
        if kind == 2 {
            attrs.insert("opacity".into(), usd::AttrValue::Float(0.2));
            attrs.insert("ior".into(), usd::AttrValue::Float(value.max(1.0)));
        }
        if kind == 3 {
            attrs.insert("emissiveColor".into(), usd::AttrValue::Vec3(color.map(|c| c * value)));
        }
        Some(path)
    }

    // The VEX an Attribute VOP network compiles to.
    pub(crate) fn vop_code(&self, id: NodeId) -> Result<String, String> {
        let nodes: HashMap<vop::NodeId, vop::VopNode> = self
            .children(id)
            .into_iter()
            .filter_map(|child| match &self.nodes.get(&child)?.kind {
                NodeKind::Vop(v) if !self.nodes[&child].bypass => Some((child, v.clone())),
                _ => None,
            })
            .collect();
        vop::compile(&nodes, |to, slot| self.input_of(to, slot).filter(|src| nodes.contains_key(src)))
    }

    // The input restricted to the node's group of primitives (all of it when there's no group).
    pub(crate) fn group_prims(&self, mesh: &Mesh, group: &groups::NodeGroup) -> Result<Mesh, String> {
        match groups::prim_mask(mesh, group)? {
            Some(prims) => Ok(groups::only_prims(mesh, &prims)),
            None => Ok(mesh.clone()),
        }
    }

    // Cooks `node` at `time` with its own cache (solvers cook inputs at other times than the frame).
    pub(crate) fn cook_fresh(&self, node: Option<NodeId>, time: f32) -> Mesh {
        node.map(|n| self.cook(n, time, &mut HashMap::new())).unwrap_or_default()
    }

    // One SOP Solver step: cooks the solver's display node with Prev_Frame bound to `prev`.
    pub(crate) fn sop_solver_step(&self, solver: NodeId, prev: &Mesh, time: f32, time_inc: f32) -> Mesh {
        let Some(display) = self.display_of(solver) else {
            return prev.clone();
        };
        SOLVER_PREV_FRAME.with(|frames| frames.borrow_mut().insert(solver, prev.clone()));
        let outer_inc = SOLVER_TIME_INC.with(|inc| inc.replace(time_inc));
        let out = self.cook(display, time, &mut HashMap::new());
        SOLVER_TIME_INC.with(|inc| inc.set(outer_inc));
        SOLVER_PREV_FRAME.with(|frames| frames.borrow_mut().remove(&solver));
        out
    }

    // One POP Network step: cooks the network's display node on `prev` (see pop.rs).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pop_step(&self, net: NodeId, prev: &Mesh, inputs: &[Mesh], time: f32, dt: f32, step: usize, frame_start: bool) -> Mesh {
        let Some(display) = self.display_of(net) else {
            return prev.clone();
        };
        let outer_inc = SOLVER_TIME_INC.with(|inc| inc.replace(dt));
        // Static Objects and Ground Planes wired into the network, for the solvers.
        let mut colliders = Vec::new();
        let (mut stack, mut seen) = (vec![display], HashSet::new());
        while let Some(at) = stack.pop() {
            if !seen.insert(at) {
                continue;
            }
            if self.nodes.get(&at).is_some_and(|n| !n.bypass) {
                if let NodeKind::Pop(p) = self.kind_at(at, time).as_ref() {
                    if matches!(p.op, pop::STATIC_OBJECT | pop::GROUND_PLANE) {
                        colliders.extend(pop::Collider::of(p, inputs));
                    }
                }
            }
            stack.extend(self.links.iter().filter(|l| l.to == at).map(|l| l.from));
        }
        let s = pop::Step { time, dt, step, frame_start, node_seed: 0, inputs, colliders: &colliders };
        let out = self.pop_eval(display, prev, &s, None, &mut HashSet::new());
        SOLVER_TIME_INC.with(|inc| inc.set(outer_inc));
        out
    }

    // The particles a node inside a POP Network gives. `onto`: the particles it acts on when
    // it's in a solver's second or third input (or below such a node); None in the main
    // stream, where a POP with nothing above it acts on the last step's particles.
    pub(crate) fn pop_eval(&self, id: NodeId, prev: &Mesh, s: &pop::Step, onto: Option<Mesh>, visiting: &mut HashSet<NodeId>) -> Mesh {
        let Some(node) = self.nodes.get(&id) else {
            return onto.unwrap_or_else(|| prev.clone());
        };
        // A loop in the wiring: stop here.
        if !visiting.insert(id) {
            return onto.unwrap_or_else(|| prev.clone());
        }
        let mut inputs: Vec<(usize, NodeId)> = self.links.iter().filter(|l| l.to == id).map(|l| (l.slot, l.from)).collect();
        inputs.sort_unstable();
        let input = |slot: usize| inputs.iter().find(|(k, _)| *k == slot).map(|(_, from)| *from);
        let mut upstream = |slot: usize, onto: Option<Mesh>, visiting: &mut HashSet<NodeId>| match input(slot) {
            Some(from) => self.pop_eval(from, prev, s, onto, visiting),
            None => onto.unwrap_or_else(|| prev.clone()),
        };
        let kind = self.kind_at(id, s.time);
        let out = match kind.as_ref() {
            _ if node.bypass => upstream(0, onto, visiting),
            // Each input in turn: the first gives the particles, the rest act on them.
            NodeKind::Merge => {
                let mut particles = onto;
                for (_, from) in &inputs {
                    particles = Some(self.pop_eval(*from, prev, s, particles, visiting));
                }
                particles.unwrap_or_else(|| prev.clone())
            }
            NodeKind::Pop(p) if p.op == pop::OBJECT => onto.unwrap_or_else(|| prev.clone()),
            NodeKind::Pop(p) if p.op == pop::SOLVER => {
                let objects = upstream(0, onto, visiting);
                let mut particles = match input(1) {
                    Some(pre) => self.pop_eval(pre, prev, s, Some(objects), visiting),
                    None => objects,
                };
                pop::integrate(&mut particles, s.dt);
                if !s.colliders.is_empty() {
                    pop::bounce_off(&mut particles, s.colliders, None);
                }
                match input(2) {
                    Some(post) => self.pop_eval(post, prev, s, Some(particles), visiting),
                    None => particles,
                }
            }
            NodeKind::Pop(p) => {
                let mut particles = upstream(0, onto, visiting);
                pop::tidy(&mut particles);
                let mask = groups::point_mask(&particles, &node.group).ok().flatten();
                // VEX input 0 is the particles; 1-3 are the POP Network's inputs 0-2.
                let others = &s.inputs[..s.inputs.len().min(vex::INPUTS - 1)];
                match p.op {
                    pop::WRANGLE => node_wrangle(particles, others, p.text(0), s.time, 2, mask.as_deref(), Some(&node.channels)),
                    pop::VOP => match self.vop_code(id) {
                        Ok(code) => node_wrangle(particles, others, &code, s.time, 2, mask.as_deref(), Some(&node.channels)),
                        Err(_) => particles,
                    },
                    _ => {
                        if let Some(code) = p.expression_code() {
                            particles = node_wrangle(particles, others, &code, s.time, 2, None, Some(&node.channels));
                        }
                        let step = pop::Step { node_seed: (id as u32).wrapping_mul(0x9e37_79b9), ..*s };
                        pop::apply(p, &mut particles, &step, mask.as_deref());
                        particles
                    }
                }
            }
            // Anything else passes the particles through.
            _ => upstream(0, onto, visiting),
        };
        visiting.remove(&id);
        out
    }

    // Identifies a solver's simulation by everything that affects it: its settings,
    // its upstream nodes and, for containers, their networks. Node positions are excluded,
    // so moving nodes keeps cached simulations while any parameter edit restarts them.
    // A fingerprint of everything a node's cooked output depends on: its own settings, group,
    // channels, keyframes and bypass, and its inputs' fingerprints. A cooked result stays good
    // while its fingerprint does, so moving a node, the display flag or the selection keeps it.
    // Nodes that also read beyond their inputs (containers, Object Merge, files, inner networks,
    // USD...) take in `scene`, the whole scene's fingerprint, as everything did before.
    pub(crate) fn cook_fingerprints(&self, scene: u64) -> HashMap<NodeId, u64> {
        let mut memo = HashMap::new();
        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        for id in ids {
            self.cook_fingerprint(id, scene, &mut memo, &mut HashSet::new());
        }
        memo
    }

    pub(crate) fn cook_fingerprint(&self, id: NodeId, scene: u64, memo: &mut HashMap<NodeId, u64>, visiting: &mut HashSet<NodeId>) -> u64 {
        use std::hash::{Hash, Hasher};
        if let Some(f) = memo.get(&id) {
            return *f;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        let Some(node) = self.nodes.get(&id) else { return h.finish() };
        // A loop in the wiring can't be cooked; take the whole scene rather than recurse forever.
        if !visiting.insert(id) {
            return scene;
        }
        format!("{:?}", node.kind).hash(&mut h);
        format!("{:?}", node.group).hash(&mut h);
        format!("{:?}", node.channels).hash(&mut h);
        node.bypass.hash(&mut h);
        for (channel, keys) in self.node_channels(id) {
            format!("{:?} {:?}", channel.target, keys).hash(&mut h);
        }
        // Inside a Subnet, its promoted values can change this node's parameters.
        for subnet in self.subnets_around(id) {
            format!("{:?}", self.nodes[&subnet].kind).hash(&mut h);
            for (channel, keys) in self.node_channels(subnet) {
                format!("{:?} {:?}", channel.target, keys).hash(&mut h);
            }
        }
        let kind = &node.kind;
        let reads_beyond_inputs = kind.is_container()
            || kind.is_lop()
            || matches!(
                kind.name(),
                "geometry" | "cop_network" | "cop" | "top" | "top_network" | "solver_input" | "foreach_begin" | "foreach_end" | "vdb" | "usd_export" | "object_merge"
                    | "sop_solver" | "compile_begin" | "compile_end" | "attribute_vop" | "vop" | "file" | "cache" | "popnet" | "pop" | "subnet_input"
            );
        if reads_beyond_inputs {
            scene.hash(&mut h);
        }
        let mut inputs: Vec<(usize, usize, NodeId)> = self.links.iter().filter(|l| l.to == id).map(|l| (l.slot, l.out, l.from)).collect();
        inputs.sort_unstable();
        for (slot, out, from) in inputs {
            (slot, out, self.cook_fingerprint(from, scene, memo, visiting)).hash(&mut h);
        }
        visiting.remove(&id);
        let f = h.finish();
        memo.insert(id, f);
        f
    }

    pub(crate) fn sim_key(&self, id: NodeId) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        self.hash_upstream(id, &mut hasher, &mut HashSet::new());
        hasher.finish()
    }

    pub(crate) fn hash_upstream(&self, id: NodeId, hasher: &mut std::collections::hash_map::DefaultHasher, seen: &mut HashSet<NodeId>) {
        use std::hash::Hash;
        id.hash(hasher);
        if !seen.insert(id) {
            return;
        }
        let Some(node) = self.nodes.get(&id) else { return };
        format!("{:?}", node.kind).hash(hasher);
        format!("{:?}", node.group).hash(hasher);
        format!("{:?}", node.channels).hash(hasher);
        node.bypass.hash(hasher);
        let mut inputs: Vec<&Link> = self.links.iter().filter(|l| l.to == id).collect();
        inputs.sort_by_key(|l| l.slot);
        for link in inputs {
            link.slot.hash(hasher);
            link.out.hash(hasher);
            self.hash_upstream(link.from, hasher, seen);
        }
        if node.kind.is_container() {
            self.display_of(id).hash(hasher);
            for child in self.children(id) {
                self.hash_upstream(child, hasher, seen);
            }
        }
    }

    pub(crate) fn time_dependent(&self, id: NodeId) -> bool {
        let Some(n) = self.nodes.get(&id) else {
            return false;
        };
        (!n.bypass && matches!(
            n.kind,
            NodeKind::Noise { .. }
                | NodeKind::Ripple { .. }
                | NodeKind::Particles { .. }
                | NodeKind::PopNet { .. }
                | NodeKind::LoopParticles { .. }
                | NodeKind::MakeLoop { .. }
                | NodeKind::TimeShift { .. }
                | NodeKind::TimeWarp { .. }
                | NodeKind::TimeBlend
                | NodeKind::Trail { .. }
                | NodeKind::Flip { .. }
                | NodeKind::WhitewaterSolver(_)
                | NodeKind::CrowdSolver(_)
                | NodeKind::MuscleSolver(_)
                | NodeKind::ProceduralMotion(_)
                | NodeKind::RbdBullet(_)
                | NodeKind::Cache { .. }
                | NodeKind::Pyro(_)
                | NodeKind::RbdSolver(_)
                | NodeKind::VellumSolver(_)
                | NodeKind::SopSolver { .. }
                | NodeKind::SolverInput { .. }
        ))
            || matches!(&n.kind, NodeKind::Wrangle { code, .. } | NodeKind::VolumeWrangle { code } | NodeKind::HeightfieldWrangle { code } if ["@Time", "@Frame", "@SimTime", "@SimFrame"].iter().any(|t| code.contains(t)))
            || matches!(&n.kind, NodeKind::TestGeometry(t) if t.shape == testgeo::CHARACTER)
            || (matches!(n.kind, NodeKind::AttributeVop { .. })
                && self.vop_code(id).is_ok_and(|code| ["@Time", "@Frame", "@SimTime", "@SimFrame"].iter().any(|t| code.contains(t))))
            || matches!(&n.kind, NodeKind::Cop(c) if c.time_dependent())
            || self.is_animated(id)
            || match &n.kind {
                NodeKind::LopSopImport { geometry: Some(g), .. } => self.time_dependent(*g),
                NodeKind::LopSopImport { geometry: None, .. } => self.geometry_ids().into_iter().any(|g| self.time_dependent(g)),
                NodeKind::LopReference { file, .. } => usd::is_animated(file),
                NodeKind::FileImport { file, .. } => {
                    fileio::has_frame_token(file)
                        || (matches!(fileio::extension(file).as_str(), "usd" | "usda" | "usdc" | "usdz") && usd::is_animated(file))
                        || (fileio::extension(file) == "abc" && abc::is_animated(file.trim()))
                        || (fileio::extension(file) == "fbx" && fileio::fbx_is_animated(file.trim()))
                }
                _ => false,
            }
            || (!n.bypass
                && matches!(n.kind, NodeKind::Geometry { .. } | NodeKind::CopNetwork | NodeKind::Subnet { .. })
                && self.display_of(id).is_some_and(|d| self.time_dependent(d)))
            // A Subnet Input changes when what feeds the Subnet does.
            || (matches!(n.kind, NodeKind::SubnetInput { .. })
                && n.parent.is_some_and(|subnet| self.links.iter().filter(|l| l.to == subnet).any(|l| self.time_dependent(l.from))))
            // Animated promoted parameters animate the nodes they drive.
            || self.subnets_around(id).into_iter().any(|subnet| {
                self.is_animated(subnet) && matches!(&self.nodes[&subnet].kind, NodeKind::Subnet { spec, .. } if spec.params.iter().any(|p| p.node == id))
            })
            // Nodes inside a POP Network show its simulation.
            || n.parent.is_some_and(|p| matches!(self.nodes.get(&p).map(|n| &n.kind), Some(NodeKind::PopNet { .. })))
            || self
                .links
                .iter()
                .filter(|l| l.to == id)
                .any(|l| self.time_dependent(l.from))
    }

    pub(crate) fn disconnect(&mut self, to: NodeId, slot: usize) {
        self.links.retain(|l| !(l.to == to && l.slot == slot));
        // A Merge closes the gap: later inputs move up one.
        if self.nodes.get(&to).is_some_and(|n| n.kind.is_multi_input()) {
            for l in self.links.iter_mut().filter(|l| l.to == to && l.slot > slot) {
                l.slot -= 1;
            }
        }
    }

    // Input ports shown on a node: a Merge has one more than its connected inputs.
    pub(crate) fn input_slots(&self, id: NodeId) -> usize {
        match self.nodes.get(&id).map(|n| &n.kind) {
            // Its inputs and one free spot.
            Some(kind) if kind.is_multi_input() => self.links.iter().filter(|l| l.to == id).map(|l| l.slot + 2).max().unwrap_or(1),
            Some(kind) => kind.num_inputs(),
            None => 0,
        }
    }

    // Where a wire dropped on a node goes: a new input on a Merge, else the
    // first free input (or the first input, replacing it, when all are used).
    pub(crate) fn drop_slot(&self, id: NodeId) -> Option<usize> {
        let kind = &self.nodes.get(&id)?.kind;
        let used = |slot: usize| self.links.iter().any(|l| l.to == id && l.slot == slot);
        if kind.is_multi_input() {
            return Some(self.links.iter().filter(|l| l.to == id).map(|l| l.slot + 1).max().unwrap_or(0));
        }
        let n = kind.num_inputs();
        (n > 0).then(|| (0..n).find(|s| !used(*s)).unwrap_or(0))
    }

    #[cfg(feature = "native-openvdb")]
    pub(crate) fn cook_native_volume(
        &self,
        id: NodeId,
        time: f32,
        cache: &mut HashMap<NodeId, NativeVolume>,
    ) -> Option<NativeVolume> {
        if let Some(volume) = cache.get(&id) {
            return Some(volume.clone());
        }
        let node = self.nodes.get(&id)?;
        let NodeKind::Vdb { voxel_size, padding, operation, iterations, .. } = &node.kind else {
            return None;
        };
        let source = self.input_of(id, 0)?;
        let mut volume = if let Some(upstream) = self.cook_native_volume(source, time, cache) {
            upstream
        } else {
            let mut mesh_cache = HashMap::new();
            native_volume_from_mesh(
                &self.cook(source, time, &mut mesh_cache),
                *voxel_size,
                *padding,
            )?
        };
        if *operation != 0
            && unsafe {
                mini_openvdb_volume_operation(volume.0, *operation as u32, *iterations as u32)
            } == 0
        {
            return None;
        }
        cache.insert(id, volume.clone());
        Some(volume)
    }

    pub(crate) fn cook(&self, id: NodeId, time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        if let Some(m) = cache.get(&id) {
            return m.clone();
        }
        // Timed for the performance monitor (cache hits above aren't).
        let _timer = perf::start(id);
        self.cook_uncached(id, time, cache)
    }

    pub(crate) fn cook_uncached(&self, id: NodeId, time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let Some(node) = self.nodes.get(&id) else {
            return Mesh::default();
        };
        let input = |slot: usize, cache: &mut HashMap<NodeId, Mesh>| {
            self.input_link(id, slot)
                .map(|(src, out)| self.cook_output(src, out, time, cache))
                .unwrap_or_default()
        };
        // A node inside a POP Network (a POP, or a Merge of POPs) shows the network's particles.
        if let Some(net) = node.parent.filter(|p| matches!(self.nodes.get(p).map(|n| &n.kind), Some(NodeKind::PopNet { .. }))) {
            let out = self.cook(net, time, cache);
            cache.insert(id, out.clone());
            return out;
        }
        let out = if node.bypass {
            if node.kind.num_inputs() > 0 {
                input(0, cache)
            } else {
                Mesh::default()
            }
        } else {
            let animated = self.kind_at(id, time);
            match animated.as_ref() {
                NodeKind::Add { count, spacing, layout, points, delete_geometry } => {
                    let mut base = input(0, cache);
                    if *delete_geometry {
                        base.indices.clear();
                        base.point_attributes.retain(|k, _| !k.starts_with("prim:") && !k.starts_with("vertex:"));
                    }
                    let mut added = if *count > 0 { node_add(*count, *spacing, *layout) } else { Mesh::default() };
                    added.vertices.extend(points.iter().map(|p| Vec3::from_arr(*p)));
                    if base.vertices.is_empty() {
                        added
                    } else if added.vertices.is_empty() {
                        base
                    } else {
                        node_merge(base, added)
                    }
                }
                NodeKind::Grid { size, res } => node_grid(*size, *size, *res, *res),
                NodeKind::Plane { size, res } => node_grid(*size, *size, *res, *res),
                NodeKind::Cone { radius, height, rows, cols, caps } => {
                    node_cone(*radius, *height, *rows, *cols, *caps)
                }
                NodeKind::Capsule { radius, length, rows, cols } => {
                    node_capsule(*radius, *length, *rows, *cols)
                }
                NodeKind::Box { size, div } => node_box(*size, *div),
                NodeKind::Sphere { radius, rows, cols } => node_sphere(*radius, *rows, *cols),
                NodeKind::Torus { major, minor, rows, cols } => {
                    node_torus(*major, *minor, *rows, *cols)
                }
                NodeKind::Transform { t, r, s } => node_transform(input(0, cache), *t, *r, *s),
                NodeKind::Noise { freq, amp } => {
                    node_noise_displace(input(0, cache), *freq, *amp, time)
                }
                NodeKind::Taper { factor } => node_taper(input(0, cache), *factor),
                NodeKind::Twist { angle } => node_twist(input(0, cache), *angle),
                NodeKind::Bend { angle } => node_bend(input(0, cache), *angle),
                NodeKind::Smooth { iterations, strength } => {
                    node_smooth(input(0, cache), *iterations, *strength)
                }
                NodeKind::Bevel { amount, segments } => {
                    node_bevel(input(0, cache), *amount, *segments)
                }
                NodeKind::Fuse { tol } => node_fuse(input(0, cache), *tol),
                NodeKind::Subdivide { levels } => node_subdivide(input(0, cache), *levels),
                NodeKind::Remesh { target } => node_remesh(input(0, cache), *target),
                NodeKind::Mirror { axis } => node_mirror(input(0, cache), *axis),
                NodeKind::Copy { count, t, r, s } => node_copy(input(0, cache), *count, *t, *r, *s),
                NodeKind::CopyToPoints { scale, rotation, piece_attribute } => {
                    let copies = node_copy_pieces_to_points(input(0, cache), input(1, cache), *scale, *rotation, piece_attribute);
                    self.report(id, copies)
                }
                NodeKind::AttributeCreate(settings) => {
                    let result = attrib::create(input(0, cache), settings);
                    self.report(id, result)
                }
                NodeKind::AttributeRandomize(settings) => {
                    let result = randomize::randomize(input(0, cache), settings);
                    self.report(id, result)
                }
                NodeKind::AttributeDelete(settings) => attrib::delete(input(0, cache), settings),
                NodeKind::AttributeVop { domain } => match self.vop_code(id) {
                    Ok(code) => {
                        let source = input(0, cache);
                        let mask = match *domain {
                            2 => groups::point_mask(&source, &node.group),
                            1 => groups::prim_mask(&source, &node.group),
                            3 => groups::prim_mask(&source, &node.group).map(|m| m.map(|m| m.iter().flat_map(|&b| [b; 3]).collect())),
                            _ => Ok(None),
                        };
                        match mask {
                            Ok(mask) => {
                                node_errors(|e| e.remove(&id));
                                let others: Vec<Mesh> = (1..vex::INPUTS).map(|slot| input(slot, cache)).collect();
                                node_wrangle(source, &others, &code, time, *domain, mask.as_deref(), Some(&node.channels))
                            }
                            Err(e) => self.report(id, Err(e)),
                        }
                    }
                    Err(e) => self.report(id, Err(e)),
                },
                // VOPs only exist to be compiled by their Attribute VOP.
                NodeKind::Vop(_) => Mesh::default(),
                NodeKind::Wrangle { code, domain } => {
                    let source = input(0, cache);
                    // Points (2) and primitives (1) run on the group's members; vertices on its primitives'.
                    let mask = match *domain {
                        2 => groups::point_mask(&source, &node.group),
                        1 => groups::prim_mask(&source, &node.group),
                        3 => groups::prim_mask(&source, &node.group).map(|m| m.map(|m| m.iter().flat_map(|&b| [b; 3]).collect())),
                        _ => Ok(None),
                    };
                    let others: Vec<Mesh> = (1..vex::INPUTS).map(|slot| input(slot, cache)).collect();
                    match mask {
                        Ok(mask) => node_wrangle(source, &others, code, time, *domain, mask.as_deref(), Some(&node.channels)),
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::VolumeWrangle { code } => {
                    let inc = SOLVER_TIME_INC.with(|inc| inc.get());
                    let others: Vec<Mesh> = (1..vex::INPUTS).map(|slot| input(slot, cache)).collect();
                    self.report(id, fields::volume_wrangle(input(0, cache), &others, code, time, inc, Some(&node.channels)))
                }
                NodeKind::HeightfieldWrangle { code } => {
                    let source = input(0, cache);
                    let inc = SOLVER_TIME_INC.with(|inc| inc.get());
                    let others: Vec<Mesh> = (1..vex::INPUTS).map(|slot| input(slot, cache)).collect();
                    match groups::point_mask(&source, &node.group) {
                        Ok(mask) => self.report(id, fields::heightfield_wrangle(source, &others, code, time, inc, mask.as_deref(), Some(&node.channels))),
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::LSystem(settings) => self.report(id, lsystem::generate(settings)),
                NodeKind::TestGeometry(settings) => self.report(id, testgeo::generate(settings, time)),
                NodeKind::Merge => {
                    // Every connected input, in order.
                    let mut slots: Vec<usize> = self.links.iter().filter(|l| l.to == id).map(|l| l.slot).collect();
                    slots.sort_unstable();
                    slots.dedup();
                    slots.into_iter().fold(Mesh::default(), |acc, slot| node_merge(acc, input(slot, cache)))
                }
                NodeKind::Null => input(0, cache),
                NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                    node_tube(*r_bot, *r_top, *height, *rows, *cols, *caps)
                }
                NodeKind::Peak { dist } => node_peak(input(0, cache), *dist),
                NodeKind::Ripple { freq, amp, speed } => {
                    node_ripple(input(0, cache), *freq, *amp, *speed, time)
                }
                NodeKind::Jitter { amp, seed, scale } => node_jitter(input(0, cache), *amp, *seed as u32, *scale),
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
                    let settings = sims::ParticleSettings {
                        max_particles: *count,
                        rate: *emission_rate,
                        lifetime: *lifetime,
                        speed: *speed,
                        spread: *spread,
                        gravity: *gravity,
                        drag: *drag,
                        bounce: *bounce,
                        friction: *friction,
                        radius: *radius,
                        substeps: *substeps,
                        along_normals: *along_normals,
                        seed: *seed as u32,
                    };
                    let (source, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = source.is_some_and(|s| self.time_dependent(s));
                    sims::particles(
                        self.sim_key(id),
                        &settings,
                        time,
                        animated,
                        |t| self.cook_fresh(source, t),
                        || self.cook_fresh(collider, 0.0),
                    )
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
                } => node_flip(
                    self.sim_key(id),
                    input(0, cache),
                    || self.cook_fresh(self.input_of(id, 1), 0.0),
                    *particle_count,
                    *grid_resolution,
                    *flip_ratio,
                    *viscosity,
                    *gravity,
                    *max_dt,
                    *substeps,
                    *seed,
                    *domain,
                    time,
                ),
                NodeKind::WhitewaterSolver(settings) => {
                    let (fluid, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    if fluid.is_none() {
                        self.report(id, Err("connect a FLIP fluid to input 1".to_owned()))
                    } else {
                        // Reported, so the "connect" error from before it was wired goes away.
                        let mesh = whitewater::whitewater(self.sim_key(id), settings, time, |t| self.cook_fresh(fluid, t), || self.cook_fresh(collider, 0.0));
                        self.report(id, Ok(mesh))
                    }
                }
                NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                    node_particle_surface(
                        input(0, cache),
                        *radius,
                        *join_distance,
                        *resolution,
                        *smoothing,
                    )
                }
                NodeKind::Extrude(settings) => {
                    let source = input(0, cache);
                    match groups::prim_mask(&source, &node.group) {
                        Ok(mask) => extrude::extrude(&source, mask.as_deref(), settings),
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::Clip { axis, offset, invert } => node_clip(input(0, cache), *axis, *offset, *invert),
                NodeKind::Blast { component, indices, invert } if !node.group.is_empty() => {
                    // Blast deletes the node's group (points or primitives).
                    let source = input(0, cache);
                    let as_points = groups::guess_domain(&source, &node.group.pattern) == groups::GroupDomain::Points && node.group.kind != 2 || node.group.kind == 1;
                    let mask = if as_points { groups::point_mask(&source, &node.group) } else { groups::prim_mask(&source, &node.group) };
                    let _ = (component, indices);
                    match mask {
                        Ok(Some(mask)) => {
                            let chosen: Vec<usize> = mask.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect();
                            self.report(id, Ok(node_blast(source, if as_points { 0 } else { 1 }, &chosen, *invert)))
                        }
                        Ok(None) => source,
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::Blast { component, indices, invert } => {
                    node_blast(input(0, cache), *component, indices, *invert)
                }
                NodeKind::Cache { cache_id, mode, .. } => {
                    let path = cache_file_path(*cache_id, time);
                    if *mode == 0 {
                        load_mesh_cache(&path).unwrap_or_else(|| input(0, cache))
                    } else {
                        let mesh = input(0, cache);
                        if let Err(error) = save_mesh_cache(&path, &mesh) {
                            eprintln!("Could not write cache {}: {error}", path.display());
                        }
                        mesh
                    }
                }
                NodeKind::Vdb { voxel_size, padding, operation, iterations, output: 1 } => {
                    let result = self.vdb_volume(id, input(0, cache), time, *voxel_size, *padding, *operation, *iterations);
                    self.report(id, result)
                }
                NodeKind::ConvertVdb { mode, iso } => {
                    let source = input(0, cache);
                    let result = volume::from_mesh(&source).map(|v| match mode {
                        0 => volume::to_polygons(&v, *iso),
                        _ => volume::to_mesh(&volume::to_fog(&v)),
                    });
                    self.report(id, result)
                }
                NodeKind::FileImport { file, take, looping } => {
                    let path = fileio::expand_frame(file.trim(), time, sims::SIM_FPS);
                    let result = if fileio::extension(&path) == "fbx" { fileio::read_fbx_take(&path, take, time, *looping) } else { read_geometry_file(&path, time) };
                    self.report(id, result)
                }
                NodeKind::FileExport { .. } => input(0, cache),
                NodeKind::Vdb { voxel_size, padding, operation, iterations, .. } => {
                    #[cfg(feature = "native-openvdb")]
                    {
                        let mut volume_cache = HashMap::new();
                        if let Some(volume) = self.cook_native_volume(id, time, &mut volume_cache) {
                            if let Some(mesh) = native_volume_to_mesh(&volume) {
                                mesh
                            } else {
                                node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                            }
                        } else {
                            node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                        }
                    }
                    #[cfg(not(feature = "native-openvdb"))]
                    {
                        node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                    }
                }
                NodeKind::MoonRayRender { .. } => input(0, cache),
                NodeKind::Camera { .. }
                | NodeKind::Light { .. }
                | NodeKind::EnvironmentLight { .. }
                | NodeKind::Material { .. }
                | NodeKind::Shader(_)
                | NodeKind::MaterialBuilder
                | NodeKind::ShaderNode(_)
                | NodeKind::TopNetwork
                | NodeKind::Top(_) => Mesh::default(),
                NodeKind::CopNetwork => match self.display_of(id) {
                    Some(display) => self.cook(display, time, cache),
                    None => Mesh::default(),
                },
                NodeKind::Cop(cop) => {
                    let images: Vec<Option<cops::Image>> = (0..cop.def().inputs)
                        .map(|slot| self.input_of(id, slot).and_then(|_| cops::Image::from_mesh(&input(slot, cache))))
                        .collect();
                    let file = match cop.op {
                        cops::FILE => cop.texts.first().map(|f| fileio::expand_frame(f.trim(), time, sims::SIM_FPS)),
                        cops::RENDER => cops::last_render(),
                        _ => None,
                    };
                    self.report(id, cops::cook(cop, &images, file.as_deref(), time).map(|img| img.to_mesh()))
                }
                NodeKind::Geometry { .. } => match self.display_of(id) {
                    Some(display) => self.xform_at(id, time).apply(self.cook(display, time, cache)),
                    None => Mesh::default(),
                },
                // A Subnet's output is its display node inside.
                NodeKind::Subnet { .. } => self.display_of(id).map(|display| self.cook(display, time, cache)).unwrap_or_default(),
                NodeKind::SubnetInput { index } => match node.parent.filter(|p| matches!(self.nodes.get(p).map(|n| &n.kind), Some(NodeKind::Subnet { .. }))) {
                    Some(subnet) => self.input_link(subnet, *index).map(|(src, out)| self.cook_output(src, out, time, cache)).unwrap_or_default(),
                    None => Mesh::default(),
                },
                NodeKind::Reverse => node_reverse(input(0, cache)),
                NodeKind::LopSopImport { .. }
                | NodeKind::LopReference { .. }
                | NodeKind::LopPrimitive { .. }
                | NodeKind::LopTransform { .. }
                | NodeKind::LopLight { .. }
                | NodeKind::LopCamera { .. }
                | NodeKind::LopMaterial { .. }
                | NodeKind::LopPrune { .. }
                | NodeKind::LopMerge
                | NodeKind::LopExport { .. } => self.cook_stage(id, time, &mut HashMap::new()).to_mesh(),
                NodeKind::TimeShift { offset, hold, at } => self.cook_input_fresh(id, 0, if *hold { *at } else { time + offset }),
                NodeKind::TimeWarp { speed, anchor } => self.cook_input_fresh(id, 0, anchor + (time - anchor) * speed),
                NodeKind::TimeBlend => looping::time_blend(|t| self.cook_input_fresh(id, 0, t), time),
                NodeKind::Trail { samples, step, mode, fade } => looping::trail(|t| self.cook_input_fresh(id, 0, t), time, *samples, *step, *mode, *fade),
                NodeKind::LoopParticles { start, length } => self.report(id, looping::loop_particles(|t| self.cook_input_fresh(id, 0, t), time, *start, *length)),
                NodeKind::MakeLoop { start, length, blend } => self.report(id, looping::make_loop(|t| self.cook_input_fresh(id, 0, t), time, *start, *length, *blend)),
                NodeKind::Pyro(settings) => {
                    let (source, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = source.is_some_and(|s| self.time_dependent(s));
                    sims::pyro(
                        self.sim_key(id),
                        settings,
                        time,
                        animated,
                        |t| self.cook_fresh(source, t),
                        || self.cook_fresh(collider, 0.0),
                    )
                }
                NodeKind::HairGuides(settings) => match self.group_prims(&input(0, cache), &node.group) {
                    Ok(skin) => groom::hair_guides(&skin, settings),
                    Err(e) => self.report(id, Err(e)),
                },
                NodeKind::GuideProcess(settings) => groom::guide_process(&input(0, cache), settings),
                NodeKind::HairGenerate(settings) => match self.group_prims(&input(0, cache), &node.group) {
                    Ok(skin) => groom::hair_generate(&skin, &input(1, cache), settings),
                    Err(e) => self.report(id, Err(e)),
                },
                NodeKind::HairDeform(settings) => {
                    // The groom and skin at the rest frame, and the skin now.
                    let rest_time = (settings.rest_frame - 1.0) / sims::SIM_FPS;
                    let (hair, skin) = (self.input_of(id, 0), self.input_of(id, 1));
                    let result = if skin.is_none() {
                        Err("connect the animated skin to input 2".to_owned())
                    } else {
                        groom::hair_deform(&self.cook_fresh(hair, rest_time), &self.cook_fresh(skin, rest_time), &input(1, cache))
                    };
                    self.report(id, result)
                }
                NodeKind::Normal(settings) => sops::normal(&input(0, cache), settings),
                NodeKind::GroupCreate(settings) => self.report(id, groups::group_create(&input(0, cache), settings)),
                NodeKind::AttributePaint { attribute, values, settings } => {
                    self.report(id, paint::attribute_paint(&input(0, cache), attribute, values, settings))
                }
                NodeKind::Scatter(settings) => {
                    let result = self.group_prims(&input(0, cache), &node.group).and_then(|surface| sops::scatter(&surface, settings));
                    self.report(id, result)
                }
                NodeKind::Boolean(settings) => {
                    let result = if self.input_of(id, 1).is_none() {
                        Err("connect the cutting geometry (B) to the right input".to_owned())
                    } else {
                        boolean::boolean(&input(0, cache), &input(1, cache), settings)
                    };
                    self.report(id, result)
                }
                NodeKind::RbdBullet(settings) => {
                    let (geo, glue, proxy) = bullet::simulate(self.sim_key(id), settings, time, || {
                        (self.cook_input_fresh(id, 0, 0.0), self.cook_input_fresh(id, 1, 0.0), self.cook_input_fresh(id, 2, 0.0), self.cook_input_fresh(id, 3, 0.0))
                    });
                    cache.insert(out_key(id, 1), glue);
                    cache.insert(out_key(id, 2), proxy);
                    geo
                }
                NodeKind::RbdConfigure(settings) => {
                    let geo = input(0, cache);
                    let (glue, proxy) = (input(1, cache), input(2, cache));
                    cache.insert(out_key(id, 1), glue);
                    cache.insert(out_key(id, 2), proxy);
                    match groups::point_mask(&geo, &node.group) {
                        Ok(mask) => bullet::configure(&geo, settings, mask.as_deref()),
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::Pack(settings) => self.report(id, packed::pack(&input(0, cache), settings)),
                NodeKind::Unpack { transfer } => packed::unpack(&input(0, cache), *transfer),
                NodeKind::MaterialFracture(settings) => {
                    let (geo, constraints, proxy) = (input(0, cache), input(1, cache), input(2, cache));
                    match fracture::fracture_all(&geo, &constraints, &proxy, settings) {
                        Ok((pieces, glue, shapes)) => {
                            cache.insert(out_key(id, 1), glue);
                            cache.insert(out_key(id, 2), shapes);
                            self.report(id, Ok(pieces))
                        }
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::MaterialAssign { slots } => self.report(id, material_assign(&input(0, cache), slots)),
                NodeKind::Measure(settings) => self.report(id, measure::measure(&input(0, cache), settings)),
                NodeKind::ExtractCentroid(settings) => self.report(id, sopkit::extract_centroid(&input(0, cache), settings)),
                NodeKind::MatchSize(settings) => {
                    let reference = self.input_of(id, 1).is_some().then(|| input(1, cache));
                    self.report(id, sopkit::match_size(&input(0, cache), reference.as_ref(), settings))
                }
                NodeKind::ConvertLine => self.report(id, sopkit::convert_line(&input(0, cache))),
                NodeKind::Metaball(settings) => self.report(id, sopkit::metaball(&input(0, cache), settings)),
                NodeKind::Connectivity(settings) => self.report(id, sopkit::connectivity(&input(0, cache), settings)),
                NodeKind::Resample(settings) => self.report(id, sopkit::resample(&input(0, cache), settings)),
                NodeKind::PointDeform(settings) => self.report(id, deform::point_deform(&input(0, cache), &input(1, cache), &input(2, cache), settings)),
                NodeKind::TransformPieces => {
                    let rest = self.input_of(id, 2).map(|_| input(2, cache));
                    self.report(id, deform::transform_pieces(&input(0, cache), &input(1, cache), rest.as_ref()))
                }
                NodeKind::ForEachBegin => match FOREACH_INPUT.with(|m| m.borrow().get(&id).cloned()) {
                    Some(iteration) => iteration,
                    // Viewed on its own: the first iteration.
                    None => {
                        let source = input(0, cache);
                        match self.foreach_end_of(id).and_then(|e| match &self.nodes.get(&e)?.kind {
                            NodeKind::ForEachEnd(set) => Some(set.clone()),
                            _ => None,
                        }) {
                            Some(set) if matches!(set.mode, flow::NUMBER | flow::FEEDBACK) => flow::stamp(source, 0, set.iterations, set.start),
                            Some(set) => match flow::pieces(&source, &set) {
                                Ok(pieces) => {
                                    let n = pieces.len();
                                    pieces.into_iter().next().map_or_else(Mesh::default, |p| flow::stamp(p, 0, n, 0.0))
                                }
                                Err(e) => self.report(id, Err(e)),
                            },
                            None => source,
                        }
                    }
                },
                NodeKind::ForEachEnd(settings) => {
                    let result = self.cook_foreach(id, settings, time, cache);
                    self.report(id, result)
                }
                NodeKind::CompileBegin => input(0, cache),
                NodeKind::CompileEnd { .. } => {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    self.sim_key(id).hash(&mut hasher);
                    if self.time_dependent(id) {
                        time.to_bits().hash(&mut hasher);
                    }
                    let key = hasher.finish();
                    match flow::compiled(key) {
                        Some(mesh) => mesh,
                        None => {
                            let mesh = input(0, cache);
                            flow::store_compiled(key, &mesh);
                            mesh
                        }
                    }
                }
                NodeKind::ObjectMerge { sources, into_this } => {
                    let result = self.object_merge(id, sources, *into_this, time);
                    self.report(id, result)
                }
                NodeKind::CrowdSolver(settings) => {
                    let (start, ground, obstacles) = (self.input_of(id, 0), self.input_of(id, 1), self.input_of(id, 2));
                    if start.is_none() {
                        self.report(id, Err("connect start points to input 1 (for example Scatter on a grid)".to_owned()))
                    } else {
                        // Reported, so the "connect" error from before it was wired goes away.
                        let mesh = crowd::crowd(
                            self.sim_key(id),
                            settings,
                            time,
                            || self.cook_fresh(start, 0.0),
                            || self.cook_fresh(ground, 0.0),
                            || self.cook_fresh(obstacles, 0.0),
                        );
                        self.report(id, Ok(mesh))
                    }
                }
                NodeKind::Muscle(settings) => self.report(id, muscle::muscle(&input(0, cache), settings)),
                NodeKind::Skeleton(settings) => kinefx::skeleton(settings),
                NodeKind::RigPose(pose) => self.report(id, kinefx::rig_pose(&input(0, cache), pose)),
                NodeKind::TwoBoneIk(settings) => self.report(id, kinefx::two_bone_ik(&input(0, cache), settings)),
                NodeKind::ProceduralMotion(settings) => self.report(id, kinefx::motion(&input(0, cache), settings, time)),
                NodeKind::SkeletonBlend { weight } => self.report(id, kinefx::blend(&input(0, cache), &input(1, cache), *weight)),
                NodeKind::JointCapture(settings) => self.report(id, kinefx::capture(&input(0, cache), &input(1, cache), settings)),
                NodeKind::BoneDeform => self.report(id, kinefx::bone_deform(&input(0, cache), &input(1, cache), &input(2, cache))),
                NodeKind::MuscleSolver(settings) => {
                    let source = self.input_of(id, 0);
                    muscle::jiggle(self.sim_key(id), settings, time, |t| self.cook_fresh(source, t))
                }
                NodeKind::MuscleSkin(settings) => {
                    let muscles = self.input_of(id, 1);
                    let rest = self.cook_fresh(muscles, 0.0);
                    self.report(id, muscle::skin(&input(0, cache), &rest, &input(1, cache), settings))
                }
                NodeKind::Terrain(settings) => terrain::terrain(settings),
                NodeKind::TerrainNoise(settings) => self.report(id, terrain::terrain_noise(&input(0, cache), settings)),
                NodeKind::TerrainErode(settings) => self.report(id, terrain::terrain_erode(&input(0, cache), settings)),
                NodeKind::TerrainTerrace(settings) => self.report(id, terrain::terrain_terrace(&input(0, cache), settings)),
                NodeKind::TerrainMask(settings) => self.report(id, terrain::terrain_mask(&input(0, cache), settings)),
                NodeKind::TerrainColorize(settings) => self.report(id, terrain::terrain_colorize(&input(0, cache), settings)),
                NodeKind::UvTexture(settings) => sops::uv_texture(&input(0, cache), settings),
                NodeKind::UvTransform(settings) => self.report(id, sops::uv_transform(&input(0, cache), settings)),
                NodeKind::Color(settings) => self.report(id, color::color(&input(0, cache), settings)),
                NodeKind::Enumerate(settings) => self.report(id, enumerate::enumerate(&input(0, cache), settings)),
                NodeKind::AttributePromote(settings) => self.report(id, promote::promote(&input(0, cache), settings)),
                NodeKind::AttributeBlur(settings) => {
                    // The group's points change; the rest are read but kept.
                    let source = input(0, cache);
                    let blurred = groups::point_mask(&source, &node.group).and_then(|mask| blur::blur(&source, settings, mask.as_deref()));
                    self.report(id, blurred)
                }
                NodeKind::AttributeTransfer(settings) => self.report(id, transfer::transfer(&input(0, cache), &input(1, cache), settings)),
                NodeKind::UvQuickShade { file } => self.report(id, sops::uv_quickshade(&input(0, cache), file)),
                NodeKind::VellumSolver(settings) => {
                    let (geometry, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = geometry.is_some_and(|g| self.time_dependent(g));
                    // A setup that can't move is the usual "Vellum does nothing": say why.
                    let start = self.cook_fresh(geometry, 0.0);
                    let lowest = start.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                    let note = if geometry.is_none() {
                        Some("connect the cloth, hair or softbody geometry to input 1".to_owned())
                    } else if settings.ground && lowest <= settings.thickness + 1e-4 {
                        Some(format!(
                            "the geometry starts on the ground plane (lowest point y = {lowest:.3}), so it can't fall. \
                             Move it above y = 0, or turn off Ground plane to use only the collider."
                        ))
                    } else {
                        None
                    };
                    node_notes(|notes| match note {
                        Some(n) => {
                            notes.insert(id, n);
                        }
                        None => {
                            notes.remove(&id);
                        }
                    });
                    // The node's group is pinned (on top of any f@pin).
                    let pin_group = node.group.clone();
                    let pinned = |mut mesh: Mesh| -> Mesh {
                        if let Ok(Some(mask)) = groups::point_mask(&mesh, &pin_group) {
                            let pin = mesh.point_attributes.entry("pin".into()).or_insert_with(|| vec![0.0; mask.len()]);
                            pin.resize(mask.len(), 0.0);
                            for (p, m) in pin.iter_mut().zip(mask) {
                                if m {
                                    *p = 1.0;
                                }
                            }
                        }
                        mesh
                    };
                    vellum::vellum(
                        self.sim_key(id),
                        settings,
                        time,
                        animated,
                        |t| pinned(self.cook_fresh(geometry, t)),
                        || self.cook_fresh(collider, 0.0),
                    )
                }
                NodeKind::RbdSolver(settings) => {
                    let (pieces, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    sims::rbd(self.sim_key(id), settings, time, || self.cook_fresh(pieces, 0.0), || self.cook_fresh(collider, 0.0))
                }
                NodeKind::SopSolver { substeps } => {
                    let source = self.input_of(id, 0);
                    sims::sop_solver(
                        self.sim_key(id),
                        time,
                        *substeps,
                        || self.cook_fresh(source, 0.0),
                        |prev, step_time| self.sop_solver_step(id, prev, step_time, 1.0 / (sims::SIM_FPS * (*substeps).clamp(1, 16) as f32)),
                    )
                }
                NodeKind::PopNet { substeps } => {
                    let sources: Vec<Option<NodeId>> = (0..4).map(|slot| self.input_of(id, slot)).collect();
                    let animated: Vec<bool> = sources.iter().map(|s| s.is_some_and(|s| self.time_dependent(s))).collect();
                    // Inputs that don't change are cooked once; animated ones at every step.
                    let mut inputs: Option<Vec<Mesh>> = None;
                    sims::pop_network(self.sim_key(id), time, *substeps, |prev, t, dt, step, frame_start| {
                        let inputs = match &mut inputs {
                            Some(inputs) => {
                                for k in (0..4).filter(|&k| animated[k]) {
                                    inputs[k] = self.cook_fresh(sources[k], t);
                                }
                                inputs
                            }
                            None => inputs.insert((0..4).map(|k| self.cook_fresh(sources[k], if animated[k] { t } else { 0.0 })).collect()),
                        };
                        self.pop_step(id, prev, inputs, t, dt, step, frame_start)
                    })
                }
                // Inside a POP Network, every node shows the network's particles (caught above).
                NodeKind::Pop(_) => Mesh::default(),
                NodeKind::SolverInput { index } => {
                    let solver = node.parent;
                    let prev = solver.and_then(|s| SOLVER_PREV_FRAME.with(|frames| frames.borrow().get(&s).cloned()));
                    match (index, prev) {
                        (0, Some(prev)) => prev,
                        // Input_1, or Prev_Frame outside a solver step (viewing the network): the solver's input.
                        _ => solver.and_then(|s| self.input_of(s, 0)).map(|src| self.cook(src, time, cache)).unwrap_or_default(),
                    }
                }
                NodeKind::Switch { index } => {
                    // Inputs count from 0; past the last one, the last one.
                    let last = self.links.iter().filter(|l| l.to == id).map(|l| l.slot).max().unwrap_or(0);
                    input((*index).min(last), cache)
                }
            }
        };
        let mut out = out;
        // Houdini-style Group: only the group's points/primitives keep this node's changes.
        if !node.bypass && !node.group.is_empty() && node.kind.group_support() == GroupSupport::Restrict {
            let source = input(0, cache);
            match (groups::point_mask(&source, &node.group), groups::prim_mask(&source, &node.group)) {
                (Ok(Some(points)), Ok(Some(prims))) => {
                    groups::restrict_to_group(&source, &mut out, &points, &prims);
                }
                (Err(e), _) | (_, Err(e)) => out = self.report(id, Err(e)),
                _ => {}
            }
        }
        out.ensure_builtin_ids();
        cache.insert(id, out.clone());
        out
    }

    // ---- scene file / undo snapshot format ----
    pub(crate) fn serialize(&self) -> String {
        let mut s = format!("pfs {SCENE_FORMAT}\n");
        let mut ids: Vec<_> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let n = &self.nodes[&id];
            // Animated nodes are written with their time-0 values; channels follow below.
            let kind = self.kind_at(id, 0.0);
            let params: Vec<String> = kind.params().iter().map(|p| p.to_string()).collect();
            let serialized_params = match kind.as_ref() {
                NodeKind::Wrangle { code, domain } => format!("{}:{}", domain, encode_text(code)),
                NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts } => {
                    format!(
                        "mr:{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                        encode_text(executable),
                        encode_text(arguments),
                        encode_text(output),
                        width,
                        height,
                        samples,
                        match opts.camera {
                            RenderCamera::Auto => "auto".to_string(),
                            RenderCamera::Viewport => "viewport".to_string(),
                            RenderCamera::Node(id) => format!("n{id}"),
                        },
                        opts.max_depth,
                        opts.light_samples,
                        opts.bsdf_samples,
                        opts.background[0],
                        opts.background[1],
                        opts.background[2],
                        opts.sequence as u8,
                        opts.range_start,
                        opts.range_end,
                        opts.range_step,
                        opts.stage as u8,
                        opts.volume_samples,
                        opts.hydra as u8,
                    )
                }
                kind => match kind.texts() {
                    texts if texts.is_empty() => params.join(" "),
                    texts => format!(
                        "t:{} {}",
                        texts.iter().map(|t| encode_text(t)).collect::<Vec<_>>().join("|"),
                        params.join(" ")
                    ),
                },
            };
            s += &format!(
                "node {} {} {} {} {} {} {}\n",
                id,
                n.pos.x,
                n.pos.y,
                n.bypass as u8,
                match n.context {
                    AppContext::Obj => "obj",
                    AppContext::Mat => "mat",
                    AppContext::Out => "out",
                    AppContext::Stage => "stage",
                    AppContext::Img => "img",
                    AppContext::Tasks => "tasks",
                },
                n.kind.name(),
                serialized_params
            );
        }
        for l in &self.links {
            if l.out == 0 {
                s += &format!("link {} {} {}\n", l.from, l.to, l.slot);
            } else {
                s += &format!("link {} {} {} {}\n", l.from, l.to, l.slot, l.out);
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            if let Some(parent) = self.nodes[&id].parent {
                s += &format!("parent {id} {parent}\n");
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            let x = self.xform_at(id, 0.0);
            if !x.is_identity() {
                let values: Vec<String> = [x.t, x.r, x.s].concat().iter().map(|v| v.to_string()).collect();
                s += &format!("xform {id} {}\n", values.join(" "));
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            for (name, v) in &self.nodes[&id].channels {
                s += &format!("chan {id} {} {} {} {}\n", encode_text(name), v[0], v[1], v[2]);
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            let name = self.nodes[&id].name.trim();
            if !name.is_empty() {
                s += &format!("name {id} {}\n", encode_text(name));
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            let group = &self.nodes[&id].group;
            if !group.is_empty() {
                s += &format!("group {id} {} {}\n", group.kind, encode_text(&group.pattern));
            }
        }
        let mut displays: Vec<_> = self.displays.iter().collect();
        displays.sort_unstable();
        for (container, display) in displays {
            s += &format!("display {container} {display}\n");
        }
        for (channel_id, channel) in &self.channels {
            if channel.is_empty() || !self.nodes.contains_key(&channel_id.node) {
                continue;
            }
            let (kind, index) = match channel_id.target {
                ChannelTarget::Param(i) => ("p", i),
                ChannelTarget::Xform(i) => ("x", i),
            };
            s += &format!("channel {} {kind} {index} {}\n", channel_id.node, channel.to_text());
        }
        s += &format!("next {}\n", self.next_id);
        s
    }

    pub(crate) fn deserialize(text: &str) -> Graph {
        let mut g = Graph::default();
        let mut legacy_display: Option<NodeId> = None;
        let mut parents: Vec<(NodeId, NodeId)> = Vec::new();
        let mut xforms: Vec<(NodeId, ObjectXform)> = Vec::new();
        // "pfs 2"; older files say "mhip 2" (same format) or "mhip 1".
        let is_v2 = text.lines().next().is_some_and(|l| matches!(l.trim(), "pfs 2" | "mhip 2"));
        for line in text.lines() {
            let t: Vec<&str> = line.split_whitespace().collect();
            match t.as_slice() {
                ["node", id, x, y, bp, ..] => {
                    // Determine optional context token; support both old and new formats
                    let mut ctx = AppContext::Obj;
                    let mut cursor = 5usize; // index of token after bp
                    if t.len() > cursor {
                        let tok = *t.get(cursor).unwrap_or(&"");
                        match tok {
                            "obj" => { ctx = AppContext::Obj; cursor += 1; }
                            "mat" => { ctx = AppContext::Mat; cursor += 1; }
                            "out" => { ctx = AppContext::Out; cursor += 1; }
                            "stage" => { ctx = AppContext::Stage; cursor += 1; }
                            "img" => { ctx = AppContext::Img; cursor += 1; }
                            "tasks" => { ctx = AppContext::Tasks; cursor += 1; }
                            _ => {}
                        }
                    }
                    let name = *t.get(cursor).unwrap_or(&"");
                    let rest = if t.len() > cursor + 1 { &t[cursor + 1..] } else { &[] as &[&str] };
                    if name == "wrangle" {
                        let (domain, encoded) = rest.first()
                            .and_then(|value| value.split_once(':'))
                            .map_or((0, rest.first().copied().unwrap_or("")), |(domain, encoded)| {
                                (domain.parse::<usize>().unwrap_or(0).min(3), encoded)
                            });
                        let code = decode_text(encoded).unwrap_or_default();
                        if let (Ok(id), Ok(x), Ok(y)) = (id.parse::<usize>(), x.parse::<f32>(), y.parse::<f32>()) {
                            g.nodes.insert(
                                id,
                                Node {
                                    kind: NodeKind::Wrangle { code, domain },
                                    pos: egui::pos2(x, y),
                                    bypass: *bp == "1",
                                    context: ctx,
                                    parent: None,
                                    xform: ObjectXform::default(),
                                    group: groups::NodeGroup::default(),
                                    channels: vex::Channels::new(),
                                    name: String::new(),
                                },
                            );
                            g.next_id = g.next_id.max(id + 1);
                        }
                        continue;
                    }
                    if name == "moonray_render" {
                        let fields = rest.first().and_then(|value| value.strip_prefix("mr:"))
                            .map(|value| value.split('|').collect::<Vec<_>>());
                        let decode = |index: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| decode_text(value))
                            .unwrap_or_default();
                        let parse = |index: usize, default: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(default);
                        let parse_f = |index: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| value.parse::<f32>().ok())
                            .unwrap_or(0.0)
                            .max(0.0);
                        let camera = fields.as_ref()
                            .and_then(|values| values.get(6))
                            .map_or(RenderCamera::Auto, |value| match *value {
                                "viewport" => RenderCamera::Viewport,
                                other => other.strip_prefix('n')
                                    .and_then(|id| id.parse().ok())
                                    .map_or(RenderCamera::Auto, RenderCamera::Node),
                            });
                        let defaults = RenderOpts::default();
                        let opts = RenderOpts {
                            camera,
                            max_depth: parse(7, defaults.max_depth).clamp(1, 32),
                            light_samples: parse(8, defaults.light_samples).clamp(1, 16),
                            bsdf_samples: parse(9, defaults.bsdf_samples).clamp(1, 16),
                            background: [parse_f(10), parse_f(11), parse_f(12)],
                            sequence: parse(13, 0) != 0,
                            range_start: parse_f(14),
                            range_end: fields.as_ref().and_then(|v| v.get(15)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(defaults.range_end).max(0.0),
                            range_step: fields.as_ref().and_then(|v| v.get(16)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(defaults.range_step).max(1e-4),
                            stage: parse(17, 0) != 0,
                            volume_samples: parse(18, defaults.volume_samples).clamp(1, 16),
                            hydra: parse(19, 0) != 0,
                        };
                        if let (Ok(id), Ok(x), Ok(y)) = (id.parse::<usize>(), x.parse::<f32>(), y.parse::<f32>()) {
                            g.nodes.insert(
                                id,
                                Node {
                                    kind: NodeKind::MoonRayRender {
                                        executable: decode(0),
                                        arguments: decode(1),
                                        output: decode(2),
                                        width: parse(3, 1280).max(1),
                                        height: parse(4, 720).max(1),
                                        samples: parse(5, 64).max(1),
                                        opts,
                                    },
                                    pos: egui::pos2(x, y),
                                    bypass: *bp == "1",
                                    context: ctx,
                                    parent: None,
                                    xform: ObjectXform::default(),
                                    group: groups::NodeGroup::default(),
                                    channels: vex::Channels::new(),
                                    name: String::new(),
                                },
                            );
                            g.next_id = g.next_id.max(id + 1);
                        }
                        continue;
                    }
                    // Text parameters come first as t:<hex>|<hex>.
                    let (texts, rest) = match rest.split_first() {
                        Some((first, numbers)) if first.starts_with("t:") => (
                            first[2..].split('|').map(|t| decode_text(t).unwrap_or_default()).collect::<Vec<_>>(),
                            numbers,
                        ),
                        _ => (Vec::new(), rest),
                    };
                    let p: Vec<f32> = rest.iter().filter_map(|v| v.parse().ok()).collect();
                    let kind = NodeKind::from_params(name, &p).map(|mut kind| {
                        kind.set_texts(&texts);
                        kind
                    });
                    if let (Ok(id), Ok(x), Ok(y), Some(kind)) = (
                        id.parse::<usize>(),
                        x.parse::<f32>(),
                        y.parse::<f32>(),
                        kind,
                    ) {
                        g.nodes.insert(
                            id,
                            Node { kind, pos: egui::pos2(x, y), bypass: *bp == "1", context: ctx, parent: None, xform: ObjectXform::default(), group: groups::NodeGroup::default(), channels: vex::Channels::new(), name: String::new() },
                        );
                        g.next_id = g.next_id.max(id + 1);
                    }
                }
                ["link", a, b, s] => {
                    if let (Ok(from), Ok(to), Ok(slot)) = (a.parse(), b.parse(), s.parse()) {
                        g.links.push(Link { from, to, slot, out: 0 });
                    }
                }
                ["link", a, b, s, o] => {
                    if let (Ok(from), Ok(to), Ok(slot), Ok(out)) = (a.parse(), b.parse(), s.parse(), o.parse()) {
                        g.links.push(Link { from, to, slot, out });
                    }
                }
                ["chan", id, name, x, y, z] => {
                    if let (Ok(id), Some(name)) = (id.parse::<usize>(), decode_text(name)) {
                        if let Some(node) = g.nodes.get_mut(&id) {
                            let v = [x, y, z].map(|c| c.parse::<f32>().unwrap_or(0.0));
                            node.channels.insert(name, v);
                        }
                    }
                }
                ["name", id, name] => {
                    if let (Ok(id), Some(name)) = (id.parse::<usize>(), decode_text(name)) {
                        if let Some(node) = g.nodes.get_mut(&id) {
                            node.name = name;
                        }
                    }
                }
                ["group", id, kind, pattern] => {
                    if let (Ok(id), Some(pattern)) = (id.parse::<usize>(), decode_text(pattern)) {
                        if let Some(node) = g.nodes.get_mut(&id) {
                            node.group = groups::NodeGroup { pattern, kind: kind.parse::<u8>().unwrap_or(0).min(2) };
                        }
                    }
                }
                ["parent", id, parent] => {
                    if let (Ok(id), Ok(parent)) = (id.parse(), parent.parse()) {
                        parents.push((id, parent));
                    }
                }
                ["display", container, node] => {
                    if let (Ok(container), Ok(node)) = (container.parse(), node.parse()) {
                        g.displays.insert(container, node);
                    }
                }
                ["display", d] => legacy_display = d.parse().ok(),
                ["channel", node, kind, index, keys] => {
                    if let (Ok(node), Ok(index)) = (node.parse::<usize>(), index.parse::<usize>()) {
                        let target = if *kind == "x" { ChannelTarget::Xform(index) } else { ChannelTarget::Param(index) };
                        let channel = anim::Channel::from_text(keys);
                        if !channel.is_empty() {
                            g.channels.insert(ChannelId { node, target }, channel);
                        }
                    }
                }
                ["xform", id, rest @ ..] if rest.len() == 9 => {
                    let v: Vec<f32> = rest.iter().filter_map(|t| t.parse().ok()).collect();
                    if let (Ok(id), true) = (id.parse::<usize>(), v.len() == 9) {
                        xforms.push((id, ObjectXform { t: [v[0], v[1], v[2]], r: [v[3], v[4], v[5]], s: [v[6], v[7], v[8]] }));
                    }
                }
                ["next", n] => {
                    if let Ok(n) = n.parse::<usize>() {
                        g.next_id = g.next_id.max(n);
                    }
                }
                _ => {}
            }
        }
        g.links
            .retain(|l| g.nodes.contains_key(&l.from) && g.nodes.contains_key(&l.to));
        // Render nodes live in OUT and take no inputs.
        let render_ids: Vec<NodeId> = g
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.is_render())
            .map(|(id, _)| *id)
            .collect();
        for id in &render_ids {
            if let Some(node) = g.nodes.get_mut(id) {
                node.context = AppContext::Out;
            }
        }
        g.links.retain(|l| !render_ids.contains(&l.to));
        for (id, parent) in parents {
            // A damaged file can name a node as its own parent, or its own child's child:
            // that would be a loop, which everything walking up the networks would follow
            // forever, so it's left out.
            let mut up = Some(parent);
            let mut loops = false;
            for _ in 0..=g.nodes.len() {
                match up {
                    Some(p) if p == id => {
                        loops = true;
                        break;
                    }
                    Some(p) => up = g.nodes.get(&p).and_then(|n| n.parent),
                    None => break,
                }
            }
            if g.nodes.contains_key(&parent) && !loops {
                if let Some(node) = g.nodes.get_mut(&id) {
                    node.parent = Some(parent);
                }
            }
        }
        if !is_v2 {
            // Flat v1 scene: wrap its OBJ-context SOP nodes in a single Geometry container.
            let sop_ids: Vec<NodeId> = g
                .nodes
                .iter()
                .filter(|(_, n)| {
                    n.context == AppContext::Obj
                        && !n.kind.is_object()
                        && !matches!(n.kind, NodeKind::MoonRayRender { .. })
                })
                .map(|(id, _)| *id)
                .collect();
            if !sop_ids.is_empty() {
                let top_left = sop_ids
                    .iter()
                    .filter_map(|id| g.nodes.get(id).map(|n| n.pos))
                    .reduce(|a, b| a.min(b))
                    .unwrap_or(egui::pos2(40.0, 40.0));
                let geo = g.add(NodeKind::Geometry { material: None }, top_left);
                for id in &sop_ids {
                    if let Some(node) = g.nodes.get_mut(id) {
                        node.parent = Some(geo);
                    }
                }
                if let Some(display) = legacy_display.filter(|d| sop_ids.contains(d)) {
                    g.displays.insert(geo, display);
                }
            }
        }
        g.displays.retain(|c, d| (*c == STAGE_NETWORK || g.nodes.contains_key(c)) && g.nodes.contains_key(d));
        for (id, xform) in xforms {
            if let Some(node) = g.nodes.get_mut(&id) {
                node.xform = xform;
            }
        }
        g.channels.retain(|c, _| g.nodes.contains_key(&c.node));
        g
    }
}
