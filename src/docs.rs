// The built-in documentation: one self-contained HTML page (Help → Documentation)
// generated from the app's own tables, so it lists exactly what the app has:
// every node by context and menu, VOP, shader, COP and TOP operations, the
// shader presets, and VEX (a language guide plus every built-in function).

use super::NodeKind;
use std::fmt::Write;

// What each node does, by its saved name.
const NODE_DOCS: &[(&str, &str)] = &[
    // Objects
    ("geometry", "An object. Double-click to build its geometry network (SOPs); the display node is what renders. Has a transform and a material."),
    ("camera", "A camera to look through (or render from). Position, rotation, focal length, clipping and orthographic mode."),
    ("light", "A point, distant or area light for the viewport and MoonRay."),
    ("environment_light", "HDRI image-based lighting for MoonRay, matched in the viewport's lighting and background."),
    // Generators
    ("add", "Points: explicit positions and/or a line or grid of them, added to the optional input. Can delete the input's polygons but keep its points."),
    ("grid", "A flat square grid of quads in the XZ plane."),
    ("plane", "A flat plane."),
    ("cone", "A cone, optionally capped."),
    ("capsule", "A capsule (cylinder with round ends)."),
    ("box", "A box, optionally divided."),
    ("sphere", "A UV sphere."),
    ("torus", "A torus (donut)."),
    ("tube", "A tube (cylinder), with separate top and bottom radii and optional caps."),
    ("particle_surface", "Turns particles (e.g. FLIP) into a smooth surface (a level set meshed at its zero)."),
    // Groom
    ("hair_guides", "Guide curves grown from a skin mesh: count, length, segments and droop. Painted density/length are respected."),
    ("guide_process", "Shapes guides: length, bend, frizz, curl or smooth."),
    ("hair_generate", "Dense hair interpolated from guides (input 2) over the skin (input 1), with clumping, frizz and tapered width."),
    ("hair_deform", "Moves hair with an animated skin."),
    ("attribute_paint", "Paint a float attribute (density, length, mask…) on the input in the viewport."),
    // Solvers
    ("particles", "A particle simulation: emitted from a source (input 1) with speed, spread, gravity and drag, bouncing off an optional collider (input 2)."),
    ("flip", "A FLIP fluid simulation: water filling the emitter (input 1), colliding with an optional static collider (input 2), inside a domain box (drag it in the viewport). Particles carry velocity v."),
    ("whitewater_solver", "Spray, foam and bubbles from a FLIP fluid (input 1), with groups spray/foam/bubble. Renders as small spheres."),
    ("pyro", "Smoke and fire on a voxel grid (stable fluids, buoyancy, combustion, vorticity and turbulence), emitted from the source geometry. Drag its domain box in the viewport."),
    ("rbd_solver", "Rigid bodies (rapier, Bullet-style): each connected piece, or each value of `piece`, falls and collides. Input 2: static collider."),
    ("cloth_solver", "XPBD cloth, hair, softbody or grains with a collider, a ground plane, self-collision and pinning (f@pin, a pin group or animated pins)."),
    ("sop_solver", "Runs its network once per step on the previous step's result (Prev_Frame), for custom simulations."),
    ("solver_input", "Inside a SOP Solver: Prev_Frame (the last step) or Input_1 (the solver's input)."),
    // Import / export
    ("file", "Reads geometry: OBJ, FBX, Alembic (animated .abc is read at the current time), STL, USD (usd/usda/usdc/usdz) or VDB. $F in the name reads one file per frame."),
    ("file_export", "Writes its input to OBJ, FBX, Alembic, STL, USD or VDB (the current frame, or one file per frame with $F). Alembic writes the whole frame range into one animated .abc."),
    ("extract_centroid", "One point per piece (whole geometry, piece/name attribute or connected pieces) at its average point or bounding box centre, with a piece attribute."),
    ("match_size", "Moves geometry so a side or centre per axis lines up with a target (input 2's bounding box, or a point/box you give), optionally scaling it to fit, uniformly or per axis."),
    ("convert_line", "Turns every polygon edge into a two-point line (a curve) with restlength; quad diagonals are skipped."),
    ("metaball", "Soft blobs around the input's points (pscale scales each, or one ball at the origin), blended into one surface; lower thresholds are fatter."),
    ("connectivity", "Numbers connected pieces 0, 1, 2… in a point or primitive attribute (class by default)."),
    ("test_geometry", "Ready-made shapes to start from: Spot (a cow), Bob (a rubber toy) and Blub (a fish) with their painted colours (Keenan Crane's public-domain models), the Platonic solids, a torus knot, a shader ball, and an animated character (Quaternius's public-domain human: Idle, Walk, Run, Jump, Punch, Death and Working, looping or not, in six looks), at a chosen size, on the ground or centred."),
    ("lsystem", "An L-system: a premise rewritten by rules for a number of generations, then drawn by a 3D turtle as curves with a tapering width. F draws, + - & ^ \\ / turn, [ ] branch, ! thins; several rules for one symbol pick at random. Presets: trees, a fractal plant, grass, Koch and Hilbert curves."),
    ("resample", "Evenly spaced points along curves, by maximum segment length or a number of segments; point attributes are interpolated."),
    ("object_merge", "Brings in an object's geometry (its display output) or any node inside an object, including other nodes in this network, optionally moved into this object's space. Pick sources from a menu per object."),
    // Volumes
    ("volume_wrangle", "Runs VEX over a volume's voxels: @density (fog) or @surface (level set), with @P at each voxel's centre. volumesample(), volumegradient() and volumeindex() read the volume anywhere; removepoint() switches voxels off. Positions don't move."),
    ("vdb", "VDB from polygons: a level set (or fog) volume of a closed mesh, with smoothing, dilate and erode."),
    ("convert_vdb", "Converts volumes: to polygons at an iso value, or to a fog volume."),
    // Deformers
    ("xform", "Transform: translate, rotate and scale. Drag its handles in the viewport (W move, E rotate, R scale)."),
    ("noise", "Displaces points with animated noise."),
    ("taper", "Tapers along an axis."),
    ("twist", "Twists around an axis."),
    ("bend", "Bends along an axis."),
    ("smooth", "Smooths positions (Laplacian)."),
    ("bevel", "Bevels edges."),
    ("peak", "Moves points along their normals."),
    ("ripple", "Animated ripples."),
    ("jitter", "Point Jitter: moves each point by a random offset of up to Amount, scaled per axis."),
    ("point_deform", "Makes geometry (input 1) follow points from rest (input 2) to deformed (input 3): each point follows its nearest points, bending and turning with them."),
    ("transform_pieces", "Moves each piece (piece or name attribute) rigidly with a template point's P, orient/transform and pscale (input 2), optionally relative to rest points (input 3)."),
    // Topology
    ("subdivide", "Subdivides polygons (smooth)."),
    ("remesh", "Rebuilds the mesh with even triangles of a target count."),
    ("fuse", "Merges points closer than a distance."),
    ("mirror", "Mirrors across an axis and keeps both halves."),
    ("copy", "Copies the input several times with a cumulative transform."),
    ("copy_to_points", "Copies input 1 onto the points of input 2: each copy is scaled by the point's pscale, turned by its orient (a quaternion, or one number: a turn about Y in degrees) and coloured by its Cd. Copies keep their own up direction: the points' N isn't used (set orient to tilt them)."),
    ("extrude", "Extrudes the node's group (or everything) along the normals with distance, inset and divisions; connected or individual faces. Output front, back and sides, each optionally in a group (extrudeFront, extrudeSide, extrudeBack)."),
    ("clip", "Cuts away one side of a plane."),
    ("blast", "Deletes points or primitives (the node's group, or picked in the viewport)."),
    ("scatter", "Points on a surface (or inside a closed mesh), relaxed for even spacing; they get N, pscale and sourceprim."),
    ("boolean", "Union, intersect or subtract two closed meshes (exact BSP, or volume for messy input). Faces keep their input's attributes (materials, Cd, UVs, groups; in Exact mode) and are grouped a_faces/b_faces."),
    ("cache", "Caches its input's frames to disk (cache/cache_ID_frame.mcache, every attribute) and reads them back. Write saves each frame as it cooks; a Fetch TOP pointed at it writes its frame range (or the Fetch's work items), across wedges too. Read plays the files back."),
    ("reverse", "Reverses polygon winding."),
    ("switch", "Passes one of its inputs through."),
    ("merge", "Merges any number of inputs: drop a wire anywhere on the node to add one (a free port is always shown)."),
    ("null", "Passes its input through (a named place in the network)."),
    // Flow control
    ("foreach_begin", "Start of a For-Each loop: puts out each iteration's piece, with detail attributes iteration, numiterations and value."),
    ("foreach_end", "For-Each loops (Connected Piece, Named Primitive, Point, Primitive, Number, Feedback): runs the nodes since its Begin once per iteration and merges the results, or feeds each into the next. Multithreaded inside a Compile Block."),
    ("compile_begin", "Start of a Compile Block."),
    ("compile_end", "Compile Block: the block between Begin and End cooks as a unit with its result cached, and For-Each loops inside run in parallel."),
    // Shading and destruction
    ("material", "Assigns MAT materials to groups of primitives, overriding the object's material. Later slots win."),
    ("rbd_bullet_solver", "Rigid pieces held by breakable glue: inputs geometry (piece), constraints, proxy collision shapes and static collision geometry (Material Fracture's outputs plug straight in). Outputs the moved pieces, the constraints still holding and the moved proxies."),
    ("rbd_configure", "Sets density, friction, bounce, active (static when off) and initial velocity on the pieces in its group, for the RBD Bullet Solver. Constraints and proxy pass through."),
    ("pack", "Packs each piece (or the whole input) into one point with P, transform and piece; the geometry is stored once. Viewport and renders show it unpacked; Transform and Copy to Points turn it."),
    ("unpack", "Turns packed points back into geometry at their transform, optionally carrying the points' attributes over."),
    ("material_fracture", "Breaks a closed mesh into pieces: concrete (Voronoi chunks), glass (cracks from an impact point) or wood (splinters along the grain). Outputs pieces, glue constraints between neighbours and proxy collision shapes for the RBD Bullet Solver; pieces keep the input's materials, colours and UVs; cut faces are in the inside group (give them their own material with a Material node)."),
    // Crowds, KineFX, muscles
    ("crowd_solver", "Agents that walk, flock, seek a goal, keep their distance, follow a ground mesh and avoid obstacles. Shows walking mannequins or points."),
    ("skeleton", "A skeleton (biped, chain or tail): joints as points with parent and transform."),
    ("rig_pose", "Rotates and moves joints in their own axes; children follow. Keyframe it to animate."),
    ("two_bone_ik", "Bends a root → mid → tip chain so the tip reaches a goal, with a pole for the bend."),
    ("procedural_motion", "A biped walk cycle, a wave down a chain or tail, or idle breathing."),
    ("skeleton_blend", "Blends two poses of the same skeleton."),
    ("joint_capture", "Skin weights from a mesh (input 1) to a rest skeleton (input 2), up to 4 joints per point."),
    ("bone_deform", "Linear blend skinning: captured mesh, rest skeleton, animated skeleton."),
    ("muscle", "A muscle (spindle or strap) between two points or along a curve; animatable activation bulges it, keeping its volume."),
    ("muscle_solver", "Jiggle on animated muscles; tendon tips stay pinned."),
    ("muscle_skin", "Moves a skin mesh (input 1) with the muscles under it (input 2), relative to frame 0."),
    // Terrain
    ("terrain", "A heightfield grid to shape with the other terrain nodes."),
    ("terrain_noise", "fBm, ridged, billow, cellular or warped noise heights, with blend modes; limited to the mask if asked."),
    ("terrain_erode", "Hydraulic (rivers) and thermal (talus) erosion; writes flow and debris."),
    ("terrain_terrace", "Stepped plateaus."),
    ("terrain_mask", "A 0–1 mask from height, slope, flow or debris."),
    ("terrain_colorize", "Colours sand, grass, rock and snow by height and slope."),
    ("heightfield_wrangle", "Runs VEX over a terrain's points: @height, @mask and its other attributes (x and z stay fixed). volumesample(0, \"height\", pos) and volumeindex() read neighbouring heights, e.g. to smooth. Works on its group's points."),
    // Attributes, groups, UV
    ("attribute_create", "Creates an attribute (id, pscale, Cd, orient) with a value."),
    ("attribute_randomize", "Random values for an attribute: id, pscale, Cd or orient presets, or any custom attribute on points, primitives or the detail, with 1 to 4 components between Min and Max, set or added."),
    ("attribute_delete", "Deletes an attribute."),
    ("wrangle", "Runs VEX code over points, primitives, vertices or the detail. See the VEX section."),
    ("attribute_vop", "VEX built from nodes: double-click to build a network of VOPs (see below)."),
    ("normal", "Computes normals on points or vertices, with a cusp angle and weighting."),
    ("measure", "Area, perimeter, point area, curvature (mean or Gaussian), the gradient of an attribute, volume or curve length, as attributes."),
    ("group_create", "Makes a named group by bounding box, random percentage, normal direction or expression."),
    ("uv_texture", "UV projection: planar, cylindrical, spherical or box."),
    ("uv_transform", "Moves, rotates and scales UVs."),
    ("uv_quickshade", "Shows a checker or image texture in the viewport."),
    // MAT
    ("diffuse", "Simple diffuse material."),
    ("metal", "Simple metal material."),
    ("glass", "Simple glass material."),
    ("emissive", "Simple light-emitting material."),
    ("principled_shader", "Physically based MoonRay surface (metallic, roughness, transmission, subsurface, sheen, clearcoat, emission…) with textures and dozens of presets."),
    ("toon_shader", "Cel shading: banded shadows and a toon highlight."),
    ("hair_shader", "For hair and fur curves."),
    ("pyro_shader", "For smoke and fire volumes."),
    ("material_builder", "Build a material from texture nodes wired into a Surface Output (see Shader Nodes)."),
    // OUT
    ("moonray_render", "Renders the scene with MoonRay: size, samples, camera, sequences, and the Render Viewer. It can render the STAGE (USD) instead, optionally through Hydra: hd_render with MoonRay's Hydra delegate (hdMoonray) reads the stage as USD, as USD-native apps do."),
    // STAGE
    ("sop_import", "Brings OBJ geometry objects onto the USD stage as Mesh prims."),
    ("usd_reference", "References (or sublayers) a USD file."),
    ("usd_primitive", "A USD sphere, cube, cylinder, cone or capsule."),
    ("usd_light", "A USD light."),
    ("usd_camera", "A USD camera."),
    ("usd_transform", "Transforms prims matching a pattern."),
    ("assign_material", "Binds a material to prims matching a pattern."),
    ("usd_prune", "Removes or hides prims."),
    ("usd_merge", "Merges any number of stages, each input layered over the ones before it; drop a wire on the node to add one."),
    ("usd_export", "Saves the stage to a USD file."),
    // IMG, TASKS
    ("cop_network", "A compositing network (IMG): build it from COP nodes; the viewport shows its output."),
    ("top_network", "A task graph (TASKS): build it from TOP nodes and cook to run its work items."),
];

const CONTEXTS: &[(&str, &str)] = &[
    ("OBJ", "Objects: Geometry objects (each with a SOP network inside), cameras and lights."),
    ("SOP", "Inside a Geometry object: the nodes that build and change geometry."),
    ("MAT", "Materials and shaders, including Material Builder networks."),
    ("OUT", "Render nodes (MoonRay)."),
    ("STAGE", "A USD stage built with LOP nodes."),
    ("IMG", "Compositing: COP Networks of image nodes."),
    ("TASKS", "Task graphs: TOP Networks that render, export, wedge and run commands."),
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn anchor(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

// Which context a node is made in.
fn context_of(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::CopNetwork => "IMG",
        NodeKind::TopNetwork => "TASKS",
        k if k.is_lop() => "STAGE",
        k if k.is_render() => "OUT",
        k if k.is_material() => "MAT",
        k if k.is_object() => "OBJ",
        _ => "SOP",
    }
}

pub fn node_doc(name: &str) -> &'static str {
    NODE_DOCS.iter().find(|(n, _)| *n == name).map_or("", |(_, d)| d)
}

fn section(out: &mut String, id: &str, title: &str) {
    let _ = write!(out, "<section id=\"{}\"><h2>{}</h2>", anchor(id), esc(title));
}

fn card(out: &mut String, title: &str, tags: &str, body: &str) {
    card_with_id(out, &anchor(title), title, tags, body);
}

// Where a VEX function's card is on the page (F1 in a wrangle jumps there).
pub fn vex_anchor(name: &str) -> String {
    format!("vex-{}", anchor(name))
}

fn card_with_id(out: &mut String, id: &str, title: &str, tags: &str, body: &str) {
    let _ = write!(
        out,
        "<div class=\"card\" data-search=\"{}\"><h3 id=\"{}\">{}</h3><div class=\"tags\">{}</div><p>{}</p></div>",
        esc(&format!("{title} {tags} {body}").to_lowercase()),
        id,
        esc(title),
        tags,
        esc(body)
    );
}

pub fn html(version: &str) -> String {
    let mut body = String::new();
    let mut nav = String::new();
    // Overview.
    section(&mut body, "overview", "Overview");
    body.push_str("<p>ProceduralFX Studio builds scenes from node networks. Each context has its own networks:</p><table>");
    for (name, what) in CONTEXTS {
        let _ = write!(body, "<tr><th>{name}</th><td>{}</td></tr>", esc(what));
    }
    body.push_str("</table><p>Add nodes with a right-click (type to search) or TAB. Double-click a network node to enter it; U goes back up. F2 renames the selected node; a renamed node still shows its type under the name. Some nodes have several outputs (Material Fracture, RBD Configure, RBD Bullet Solver: geometry, constraints, proxy): drag a wire from the output you want; hover a port for its name. Any parameter can be keyframed: the animation editor lists each one by name (size, radius, t.x…).</p><p><b>Node Info:</b> hold the middle mouse button on a node (or click ℹ next to its name in the parameters, or right-click → Node Info) to see what it is and makes: its type, flags, inputs, cook time and errors, then point, primitive and vertex counts, bounds, attributes by class with their sizes, and groups with how many members. It also says whether the geometry is polygons, curves, points, packed primitives, a volume or a terrain.</p><p><b>Windows → Performance Monitor</b> times every node cook: each node's own time (without its inputs), how often it cooked, its slowest and average cook and its share of the total, slowest first. Click a node to go to it; <i>Show cook times on nodes</i> writes each node's latest time next to it in the network.</p></section>");
    let _ = write!(nav, "<a href=\"#overview\">Overview</a>");

    // The AI assistant.
    section(&mut body, "ai-assistant", "AI assistant");
    body.push_str("<p>Click <b>✨ AI</b> in the toolbar and describe what you want, or a change to the current scene (e.g. <i>200 rocks of random sizes scattered on a hilly terrain</i>), then <b>Build</b>. The assistant works on a copy of the scene: it adds and wires real nodes, cooks them to check point counts and errors, looks at the result, and fixes what's wrong. When it's done, <b>Accept</b> uses the new scene (one undo step reverts it) or <b>Discard</b> drops it.</p>\
<p>Choose how it runs under <b>⚙</b>:</p><table>\
<tr><th>ProceduralFX credits</th><td>Sign in with your email; no API key needed. Pay with a monthly plan or top-ups from the same settings (payment happens in your browser). Plan credit refills monthly; top-ups never expire. The panel shows what each request cost.</td></tr>\
<tr><th>Your own Claude API key</th><td>A key from console.anthropic.com, stored only on this computer. Opus 5.5 gives the best results; Sonnet and Haiku cost less.</td></tr>\
<tr><th>Another service, or a model on your own computer</th><td>Any OpenAI-compatible server: OpenAI, Google Gemini, OpenRouter, Groq, Mistral or DeepSeek with their API key, or Ollama, LM Studio, llama.cpp or vLLM running locally (no key). <b>List models</b> fetches the server's models. Pick one that supports tool calling; turn off <i>The model can see images</i> for text-only models. For Ollama, raise the context length (e.g. OLLAMA_CONTEXT_LENGTH=32768).</td></tr>\
</table><p>Terms, privacy and refunds for credits: <a href=\"https://www.build3dstudio.com\">www.build3dstudio.com</a>.</p></section>");
    let _ = write!(nav, "<a href=\"#ai-assistant\">AI assistant</a>");

    // Nodes by context and menu.
    let templates = NodeKind::templates();
    for context in ["OBJ", "SOP", "MAT", "OUT", "STAGE", "IMG", "TASKS"] {
        let id = format!("nodes-{context}");
        section(&mut body, &id, &format!("{context} nodes"));
        let _ = write!(nav, "<a href=\"#{}\">{context} nodes</a>", anchor(&id));
        let mut seen = std::collections::HashSet::new();
        for (category, kinds) in &templates {
            let kinds: Vec<&NodeKind> = kinds
                .iter()
                .filter(|k| !matches!(k, NodeKind::Vop(_) | NodeKind::ShaderNode(_) | NodeKind::Cop(_) | NodeKind::Top(_)))
                .filter(|k| !matches!(k, NodeKind::Shader(v) if v.preset != 0))
                .filter(|k| context_of(k) == context)
                .filter(|k| seen.insert(k.menu_name()))
                .collect();
            if kinds.is_empty() {
                continue;
            }
            let _ = write!(body, "<h4>{}</h4>", esc(category));
            for kind in kinds {
                let inputs = kind.num_inputs();
                let tags = format!("<span>{}</span><span>{} input{}</span>", esc(kind.name()), inputs, if inputs == 1 { "" } else { "s" });
                let doc = kind.menu_help().unwrap_or_else(|| node_doc(kind.name()));
                card(&mut body, &kind.menu_name(), &tags, doc);
            }
        }
        body.push_str("</section>");
    }

    // Network-internal operations.
    let groups: [(&str, &str, Vec<(String, String, String)>); 4] = [
        ("vop", "VOP nodes (inside an Attribute VOP)", crate::vop::OPS.iter().map(|o| (o.label.to_owned(), o.category.to_owned(), o.help.to_owned())).collect()),
        ("shader-nodes", "Shader nodes (inside a Material Builder)", crate::shading::SHADER_OPS.iter().map(|o| (o.label.to_owned(), format!("{} inputs", o.inputs.len()), o.help.to_owned())).collect()),
        ("cop", "COP nodes (inside a COP Network)", crate::cops::OPS.iter().map(|o| (o.label.to_owned(), o.category.to_owned(), o.help.to_owned())).collect()),
        ("top", "TOP nodes (inside a TOP Network)", crate::tops::OPS.iter().map(|o| (o.label.to_owned(), format!("{} input{}", o.inputs, if o.inputs == 1 { "" } else { "s" }), o.help.to_owned())).collect()),
    ];
    for (id, title, ops) in groups {
        section(&mut body, id, title);
        let _ = write!(nav, "<a href=\"#{id}\">{}</a>", esc(title.split(" (").next().unwrap_or(title)));
        if id == "top" {
            body.push_str("<p>Paths and commands fill in <code>$F</code>/<code>$F4</code> (frame), <code>@index</code>, <code>@wedgeindex</code>, <code>@&lt;wedge attribute&gt;</code> and <code>{input}</code> (the item's input files). Select the last node and press <b>Cook</b>; each work item shows as a dot (grey waiting, yellow running, green done, red failed).</p>");
        }
        for (label, tag, help) in ops {
            card(&mut body, &label, &format!("<span>{}</span>", esc(&tag)), &help);
        }
        body.push_str("</section>");
    }

    // Shader presets.
    section(&mut body, "presets", "Material presets");
    let _ = write!(nav, "<a href=\"#presets\">Material presets</a>");
    for (table, title) in [
        (crate::shading::Table::Principled, "Principled Shader"),
        (crate::shading::Table::Toon, "Toon Shader"),
        (crate::shading::Table::Hair, "Hair Shader"),
        (crate::shading::Table::Pyro, "Pyro Shader"),
    ] {
        let names: Vec<&str> = table.presets().iter().skip(1).map(|p| p.0).collect();
        let _ = write!(body, "<h4>{title}</h4><p class=\"list\">{}</p>", esc(&names.join(" · ")));
    }
    body.push_str("</section>");

    // VEX.
    section(&mut body, "vex", "VEX");
    let _ = write!(nav, "<a href=\"#vex\">VEX</a><a href=\"#vex-functions\">VEX functions</a>");
    body.push_str(VEX_GUIDE);
    let _ = write!(body, "<h4>Quick reference</h4><pre>{}</pre></section>", esc(crate::vex::SYNTAX_HELP));
    section(&mut body, "vex-functions", "VEX functions");
    let mut categories: Vec<&str> = crate::vex::BUILTINS.iter().map(|b| b.3).collect();
    categories.dedup();
    let mut done = std::collections::HashSet::new();
    for category in categories {
        if !done.insert(category) {
            continue;
        }
        let _ = write!(body, "<h4>{}</h4>", esc(category));
        for (name, min, max, cat, doc) in crate::vex::BUILTINS.iter().filter(|b| b.3 == category) {
            let args = if min == max { format!("{min} argument{}", if *min == 1 { "" } else { "s" }) } else if *max > 100 { format!("{min}+ arguments") } else { format!("{min}–{max} arguments") };
            let _ = cat;
            card_with_id(&mut body, &vex_anchor(name), name, &format!("<span>{}</span>", esc(&args)), doc);
        }
    }
    body.push_str("</section>");

    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>ProceduralFX Studio Docs</title>
<style>
:root {{ --bg:#f7f7f5; --fg:#1d1d1f; --muted:#666; --card:#fff; --line:#e3e3df; --accent:#2a6fdb; --tag:#eef2f9; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#17181a; --fg:#e8e8e6; --muted:#9a9a98; --card:#202124; --line:#2e2f33; --accent:#79a8ff; --tag:#273042; }} }}
* {{ box-sizing:border-box }}
body {{ margin:0; font:15px/1.5 -apple-system, "Segoe UI", system-ui, sans-serif; background:var(--bg); color:var(--fg); }}
nav {{ position:fixed; top:0; left:0; bottom:0; width:230px; overflow:auto; padding:18px 14px; border-right:1px solid var(--line); background:var(--bg); }}
nav b {{ display:block; margin-bottom:10px; }}
nav a {{ display:block; color:var(--fg); text-decoration:none; padding:3px 6px; border-radius:6px; font-size:14px; }}
nav a:hover {{ background:var(--tag); }}
nav input {{ width:100%; padding:7px 9px; margin:4px 0 12px; border:1px solid var(--line); border-radius:7px; background:var(--card); color:var(--fg); font-size:14px; }}
main {{ margin-left:230px; padding:22px 32px 80px; max-width:1100px; }}
h1 {{ margin:0 0 4px; font-size:26px; }} h2 {{ margin:36px 0 10px; border-bottom:1px solid var(--line); padding-bottom:6px; }} h4 {{ margin:22px 0 8px; color:var(--muted); text-transform:uppercase; font-size:12px; letter-spacing:.06em; }}
.cards {{ display:grid; grid-template-columns:repeat(auto-fill, minmax(300px, 1fr)); gap:10px; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:9px; padding:10px 13px; margin:0 0 8px; }}
.card:has(h3:target) {{ border-color:var(--accent); box-shadow:0 0 0 2px var(--accent); }}
h3:target {{ scroll-margin-top:16px; }}
.card h3 {{ margin:0; font-size:15px; }} .card p {{ margin:4px 0 0; color:var(--fg); font-size:14px; }}
.tags span {{ display:inline-block; font-size:11.5px; background:var(--tag); color:var(--muted); border-radius:5px; padding:0 6px; margin:3px 5px 0 0; font-family:ui-monospace, Menlo, monospace; }}
table {{ border-collapse:collapse; }} th, td {{ text-align:left; padding:5px 12px 5px 0; vertical-align:top; }} th {{ font-family:ui-monospace, Menlo, monospace; }}
pre, code {{ font-family:ui-monospace, Menlo, monospace; font-size:13px; }} pre {{ background:var(--card); border:1px solid var(--line); border-radius:8px; padding:12px; overflow:auto; }}
.list {{ color:var(--fg); }} .hidden {{ display:none; }} .muted {{ color:var(--muted); }}
@media (max-width: 760px) {{ nav {{ position:static; width:auto; border-right:0; border-bottom:1px solid var(--line); }} main {{ margin-left:0; padding:16px; }} }}
</style></head>
<body>
<nav><b>ProceduralFX Studio</b><input id="q" type="search" placeholder="Search nodes and VEX…" autofocus>{nav}</nav>
<main><h1>Documentation</h1><p class="muted">Version {version}. Generated from the app, so it lists exactly what this version has.</p>{body}<p class="muted">Familiar if you've used Houdini. Houdini is a trademark of Side Effects Software Inc. ProceduralFX Studio is not affiliated with or endorsed by SideFX.</p></main>
<script>
// Cards in each section sit in a grid; search hides those that don't match every word.
document.querySelectorAll('section').forEach(sec => {{
  let grid = null;
  [...sec.children].forEach(el => {{
    if (el.classList.contains('card')) {{ if (!grid) {{ grid = document.createElement('div'); grid.className = 'cards'; el.before(grid); }} grid.appendChild(el); }}
    else grid = null;
  }});
}});
const q = document.getElementById('q');
q.addEventListener('input', () => {{
  const words = q.value.toLowerCase().split(/\s+/).filter(Boolean);
  document.querySelectorAll('.card').forEach(c => c.classList.toggle('hidden', !words.every(w => c.dataset.search.includes(w))));
  document.querySelectorAll('h4').forEach(h => {{
    const grid = h.nextElementSibling;
    if (grid && grid.classList.contains('cards')) h.classList.toggle('hidden', ![...grid.children].some(c => !c.classList.contains('hidden')));
  }});
}});
</script></body></html>"##
    )
}

const VEX_GUIDE: &str = r#"
<p>VEX runs in a <b>Wrangle</b> (or compiled from an <b>Attribute VOP</b>) once per point, primitive, vertex, or once for the whole geometry (detail), in parallel. Changes to the element's own attributes apply as you write them; functions like <code>addpoint()</code> or <code>setpointattrib()</code> take effect after every element has run.</p>
<h4>Attributes</h4>
<p><code>@P</code> (position), <code>@N</code>, <code>@Cd</code>, <code>@pscale</code> and other known names have fixed types; others take a type prefix: <code>f@mass</code>, <code>i@id</code>, <code>v@vel</code>, <code>s@name</code>, <code>p@orient</code> (vector4), <code>3@rot</code> (matrix3), <code>4@xform</code> (matrix). Read-only: <code>@ptnum</code>, <code>@primnum</code>, <code>@numpt</code>, <code>@Time</code>, <code>@Frame</code>, <code>@TimeInc</code>. Writing a new attribute creates it.</p>
<pre>@P.y += sin(@P.x * 3 + @Time) * ch("amp");   // ch() adds a slider to the node
v@vel = set(0, 1, 0);
if (@P.y < 0) removepoint(0, @ptnum);</pre>
<h4>Functions</h4>
<p>Define functions before using them. Arguments are passed by reference (changing one changes the caller's variable). A name can have several versions with different parameter types (overloads); the one whose types fit the call best runs. Recursion works.</p>
<pre>float smoothpulse(float x; float w) { return smooth(0, w, x) * (1 - smooth(1 - w, 1, x)); }
void push_up(vector p; float amount) { p.y += amount; }   // changes the caller's vector
float size(vector v) { return length(v); }
float size(float a[]) { return len(a); }                 // an overload

push_up(@P, smoothpulse(@P.x, 0.2));</pre>
<h4>Loops and arrays</h4>
<pre>int near[] = nearpoints(0, @P, 0.5, 8);
vector sum = 0;
foreach (int pt; near) sum += point(0, "P", pt);
@P = sum / max(1, len(near));</pre>
<h4>Primitives are polygons</h4>
<p>Over primitives, a wrangle runs once per polygon (a quad, an n-gon or a triangle): <code>@primnum</code>, <code>@numprim</code> and <code>nprimitives()</code> count polygons, <code>@P</code> is the polygon's centre, and what it writes covers the whole polygon. <code>primpoints()</code> lists its points in order, and <code>addprim(0, "poly", ...)</code> takes any number of points.</p>
<h4>Expressions in parameters</h4>
<p>Type an expression into any number field (click it to type) and the parameter follows it at every frame, instead of keys; typing a plain number replaces it, and right-click → <b>Delete Channel</b> removes it (and any keys), keeping the current value. The node's <b>Animation</b> section shows each parameter's expression, for editing or clearing. Expressions are VEX, with Houdini's variables <code>$F</code> (frame), <code>$FF</code> (fractional frame), <code>$T</code> (seconds) and <code>$FPS</code>.</p>
<pre>$F                    // Scatter's seed: a new pattern every frame
sin($T * 2) * 0.5     // a wobbling value
fit($F, 1, 48, 0, 10) // ramps from 0 to 10 over two seconds
rand($F) * 360        // a random angle per frame</pre>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_node_is_documented() {
        let page = html("test");
        for (_, kinds) in NodeKind::templates() {
            for kind in kinds {
                if matches!(kind, NodeKind::Vop(_) | NodeKind::ShaderNode(_) | NodeKind::Cop(_) | NodeKind::Top(_) | NodeKind::Shader(_)) {
                    continue;
                }
                assert!(kind.menu_help().is_some() || !node_doc(kind.name()).is_empty(), "no docs for {}", kind.name());
                assert!(page.contains(&format!("id=\"{}\"", anchor(&kind.menu_name()))), "{} missing from the page", kind.menu_name());
            }
        }
        for (name, ..) in crate::vex::BUILTINS {
            assert!(page.contains(&format!(">{}</h3>", esc(name))), "VEX {name}");
            assert!(page.contains(&format!("id=\"{}\"", vex_anchor(name))), "VEX {name} anchor");
        }
        assert!(page.contains("Overloads") || page.contains("overloads"));
        assert!(page.starts_with("<!doctype html>") && page.ends_with("</html>"));
    }
}
