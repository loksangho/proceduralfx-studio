// The AI assistant: describe a setup in words and Claude builds it as nodes.
//
// Three ways to pay for the model: ProceduralFX credits (sign in; the credits server
// in server/ relays to Claude and meters it), Claude with the user's own Anthropic API
// key (or ANTHROPIC_API_KEY), or any OpenAI-compatible service or local model server
// (providers.rs). The model works
// on a copy of the scene in the background, through the tools in ai.rs: it
// edits, inspects what its nodes produce, and fixes problems. The result is a
// preview the user accepts (one undo step) or discards.

use super::*;
use serde_json::{json, Value};
use std::sync::mpsc::{Receiver, Sender};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
// Refusals retry on Anthropic's recommended fallback model (fallbacks: "default").
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TURNS: usize = 16;

// The credits server: PFX_AI_SERVER when the app runs, else the one it was built with
// (PFX_AI_SERVER at build time). Without one, the credits option is hidden.
pub fn credits_server() -> Option<String> {
    std::env::var("PFX_AI_SERVER").ok().or_else(|| option_env!("PFX_AI_SERVER").map(str::to_owned)).map(|s| s.trim().trim_end_matches('/').to_owned()).filter(|s| s.starts_with("http"))
}

// (model id, label, price per million tokens: input, output, cache write, cache read)
pub const MODELS: [(&str, &str, [f64; 4]); 3] = [
    ("claude-opus-5-5", "Claude Opus 5.5 (best results)", [4.0, 20.0, 5.0, 0.20]),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5 (cheaper)", [2.0, 10.0, 2.5, 0.20]),
    ("claude-haiku-5-5", "Claude Haiku 5.5 (cheapest)", [0.10, 0.50, 0.125, 0.01]),
];

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    // ProceduralFX credits: signed in to the credits server.
    #[default]
    Credits,
    Anthropic,
    // Any OpenAI-compatible chat completions server: another company's, or your own.
    Compatible,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub provider: Provider,
    // Anthropic: the key and the Claude model.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    // Other servers: where, which model, whether it takes images, and a key per server.
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub compat_model: String,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub keys: std::collections::BTreeMap<String, String>,
    // Credits: the session from signing in, and who it's for.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub email: String,
}

// Who runs a request.
#[derive(Clone, Debug)]
pub enum Backend {
    Credits { server: String, token: String, model: String },
    Anthropic { key: String, model: String },
    Compatible { base_url: String, key: String, model: String, vision: bool },
}

impl Backend {
    pub fn model(&self) -> &str {
        match self {
            Backend::Credits { model, .. } | Backend::Anthropic { model, .. } | Backend::Compatible { model, .. } => model,
        }
    }

    // Dollars: what credits were charged, list prices for your own Claude key, None otherwise.
    pub fn cost(&self, usage: &Usage) -> Option<f64> {
        match self {
            Backend::Credits { .. } => Some(usage.charged),
            Backend::Anthropic { model, .. } => Some(usage.cost(model)),
            Backend::Compatible { .. } => None,
        }
    }
}

fn settings_file() -> Option<PathBuf> {
    Some(app_config_dir()?.join("assistant.json"))
}

impl Settings {
    pub fn load() -> Self {
        let saved: Option<Value> = settings_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok());
        let mut s: Settings = saved.clone().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        // Settings from before there was a choice used an Anthropic key.
        if saved.as_ref().is_some_and(|v| v.get("provider").is_none() && !s.api_key.is_empty()) {
            s.provider = Provider::Anthropic;
        }
        if s.provider == Provider::Credits && credits_server().is_none() {
            s.provider = if s.key().is_some() { Provider::Anthropic } else { Provider::Compatible };
        }
        if !MODELS.iter().any(|(id, ..)| *id == s.model) {
            s.model = MODELS[0].0.into();
        }
        if s.base_url.trim().is_empty() {
            s.base_url = providers::PRESETS[0].1.into();
        }
        s
    }

    // Saved readable only by the user (it holds the API key).
    pub fn save(&self) -> Result<(), String> {
        let file = settings_file().ok_or("no settings folder")?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    // The saved key, else ANTHROPIC_API_KEY.
    pub fn key(&self) -> Option<String> {
        Some(self.api_key.trim().to_owned()).filter(|k| !k.is_empty()).or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()))
    }

    fn url_key(&self) -> String {
        self.base_url.trim().trim_end_matches('/').to_owned()
    }

    // The key saved for the current server (servers on this computer usually need none).
    pub fn compat_key(&self) -> String {
        self.keys.get(&self.url_key()).cloned().unwrap_or_default()
    }

    pub fn set_compat_key(&mut self, key: String) {
        let url = self.url_key();
        if key.trim().is_empty() {
            self.keys.remove(&url);
        } else {
            self.keys.insert(url, key.trim().to_owned());
        }
    }

    // What runs the next request, or what's still missing.
    pub fn backend(&self) -> Result<Backend, String> {
        match self.provider {
            Provider::Credits => match credits_server() {
                None => Err("ProceduralFX credits aren't available in this build".into()),
                Some(_) if self.token.is_empty() => Err("Sign in to use ProceduralFX credits (⚙)".into()),
                Some(server) => Ok(Backend::Credits { server, token: self.token.clone(), model: self.model.clone() }),
            },
            Provider::Anthropic => match self.key() {
                Some(key) => Ok(Backend::Anthropic { key, model: self.model.clone() }),
                None => Err("Add your Anthropic API key first (⚙)".into()),
            },
            Provider::Compatible => {
                let base_url = self.url_key();
                if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
                    return Err("Set the server address first (⚙)".into());
                }
                if self.compat_model.trim().is_empty() {
                    return Err("Choose a model first (⚙)".into());
                }
                let key = self.compat_key();
                let needs_key = providers::PRESETS.iter().any(|(_, u, needs)| *u == base_url && *needs);
                if needs_key && key.is_empty() {
                    return Err("Add the API key for this service first (⚙)".into());
                }
                Ok(Backend::Compatible { base_url, key, model: self.compat_model.trim().to_owned(), vision: self.vision })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The request: system prompt, tools, and the loop
// ---------------------------------------------------------------------------

pub fn system_prompt() -> String {
    format!(
        "You build and edit procedural 3D setups in ProceduralFX Studio, a node-based app like Houdini.\n\n\
         The scene has geometry objects; each holds a network of geometry nodes (SOPs) wired top to bottom, and the node with the display flag is what the object shows. \
         Make changes with edit_scene, then check them with inspect_node (cook errors, point and primitive counts, bounds, attributes) and view_scene (an image of the result), and fix what's wrong before you finish. \
         A good setup uses a clear chain of well-named nodes and the display flag on its last node.\n\n\
         Facts about the app:\n\
         - Y is up. Sizes are in scene units: a default box is 1 unit across.\n\
         - Geometry is polygons (quads where natural). Primitive numbers and groups refer to polygons.\n\
         - Parameters are set by name, as listed in the catalog; vectors take arrays (t: [0, 1, 0]); text parameters take strings.\n\
         - Inputs and outputs count from 1. Nodes in different objects can't be wired; use object_merge to bring geometry across.\n\
         - Wrangles run VEX (Houdini's language): @P, @N, @Cd, @pscale, @ptnum, @primnum, @Time, @Frame, and functions like rand, noise, fit, sin, point, setpointattrib, removepoint.\n\
         - Edits to a node that's already in the scene keep its other parameters; prefer changing the existing network to rebuilding it when the user asks for a change.\n\n\
         # Effects for 2D games\n\
         For a game effect (fire, smoke, sparks, a hit, a spell, a slash and the like), start from the closest preset with load_preset and change it to match the request, rather than building from nothing. \
         Presets come with a side-on camera and the playback range they're made for; the user exports them with Render → Export Sprite Sheet. \
         The particle presets are one detail wrangle whose points carry Alpha, Cd, pscale and emission (they render as soft glowing splats); their main settings are the wrangle's user channels (listed under channels, read in VEX with ch(), and set with set_params by name), so adjust those first, then the VEX.\n\
         Presets: {}\n\n\
         When you are done, reply in one or two sentences saying what you built or changed. If the request is unclear, make a reasonable choice and say what you assumed. \
         Describe only what you checked with inspect_node or view_scene; if you couldn't confirm something (how a simulation ends up, how it looks), say so rather than describing what you expect.\n\n\
         # Geometry node catalog\n{}",
        presets::PRESETS.iter().map(|p| format!("{} ({}): {}", p.name, if p.looping { "loops" } else { "plays once" }, p.description)).collect::<Vec<_>>().join(" "),
        ai::catalog_text()
    )
}

pub fn tools() -> Value {
    json!([
        {
            "name": "edit_scene",
            "description": "Change the scene: create geometry objects and nodes, set parameters by name, wire nodes, delete them, set the display flag. Edits apply in order; ids you give new objects and nodes can be used by later edits in the same call. Existing nodes are named as in the scene description (e.g. \"scatter3\") or by number (\"#12\"). Returns what was created and any errors (the other edits still apply). Use this for every change you make.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "edits": {
                        "type": "array",
                        "description": "The edits, in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {"type": "string", "enum": ["create_object", "add_node", "set_params", "connect", "disconnect", "delete", "set_display"]},
                                "id": {"type": "string", "description": "create_object, add_node: a name for the new object or node, used to refer to it in later edits"},
                                "type": {"type": "string", "description": "add_node: the node type, as named in the catalog"},
                                "object": {"type": "string", "description": "add_node: the geometry object to put the node in (an id from this call, or an existing object's name)"},
                                "name": {"type": "string", "description": "create_object, add_node: optional display name"},
                                "node": {"type": "string", "description": "set_params, disconnect, delete, set_display: the node"},
                                "params": {"type": "object", "description": "add_node, set_params: parameter values by name"},
                                "group": {"type": "string", "description": "add_node, set_params: optional group pattern the node works on, e.g. \"top\" or \"@P.y>0\""},
                                "from": {"type": "string", "description": "connect: the source node"},
                                "to": {"type": "string", "description": "connect: the destination node"},
                                "input": {"type": "integer", "description": "connect, disconnect: input number, from 1 (default 1)"},
                                "output": {"type": "integer", "description": "connect: output number, from 1 (default 1)"}
                            },
                            "required": ["op"]
                        }
                    }
                },
                "required": ["edits"]
            }
        },
        {
            "name": "inspect_node",
            "description": "Cook a node (or an object's displayed output) and report what it produces: errors, point and primitive counts, bounds, attributes and groups. Call it after building or changing something to check it worked, and before you finish. For simulations and animation, also inspect a later frame to see that things move as intended.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "node": {"type": "string", "description": "A node or object name, or #id"},
                    "frame": {"type": "number", "description": "Frame to cook at (24 per second, starting at 1); default: the current frame"}
                },
                "required": ["node"]
            }
        },
        {
            "name": "view_scene",
            "description": "See the scene: renders it like the viewport and returns the image (lit from the camera, on a grid at y = 0 with the X red, Y green and Z blue axes; colours from Cd; particles as dots, curves as lines). Use it after building something and after big changes, to check what numbers can't show: whether things sit on the ground or float, overlap, look the right shape and size, or a simulation looks right at a later frame. Look from more than one view when it matters.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "node": {"type": "string", "description": "An object or node to show on its own (default: every object in the scene)"},
                    "view": {"type": "string", "enum": snapshot::VIEWS, "description": "Camera angle (default: perspective)"},
                    "azimuth": {"type": "number", "description": "Instead of a named view: degrees around the vertical axis"},
                    "elevation": {"type": "number", "description": "Instead of a named view: degrees looking down (negative looks up)"},
                    "frame": {"type": "number", "description": "Frame to show (24 per second, starting at 1); default: the current frame"},
                    "outlines": {"type": "boolean", "description": "Draw polygon outlines (to check topology)"}
                }
            }
        },
        {
            "name": "load_preset",
            "description": "Add a starter effect for 2D games to the scene: its geometry object, ready to tweak, and a side-on camera that frames it for sprite-sheet export. Returns the new nodes and their settings. Use it as the starting point for game effects.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "enum": presets::PRESETS.iter().map(|p| p.name).collect::<Vec<_>>(), "description": "The preset"}
                },
                "required": ["name"]
            }
        },
        {
            "name": "describe_scene",
            "description": "List the scene's objects and their nodes, changed parameters, wiring and display flags, as they are now.",
            "input_schema": {"type": "object", "properties": {}}
        }
    ])
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    // Credits: the amount charged (in the server's currency), and the balance after the last call.
    pub charged: f64,
    pub balance: Option<f64>,
}

impl Usage {
    fn add(&mut self, usage: &Value) {
        let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        self.input += n("input_tokens");
        self.output += n("output_tokens");
        self.cache_write += n("cache_creation_input_tokens");
        self.cache_read += n("cache_read_input_tokens");
    }

    fn add_billing(&mut self, billing: &Value) {
        self.charged += billing["charged"].as_f64().unwrap_or(0.0);
        if let Some(b) = billing["balance"].as_f64() {
            self.balance = Some(b);
        }
    }

    // Dollars at the model's list prices.
    pub fn cost(&self, model: &str) -> f64 {
        let p = MODELS.iter().find(|(id, ..)| *id == model).map_or(MODELS[0].2, |m| m.2);
        (self.input as f64 * p[0] + self.output as f64 * p[1] + self.cache_write as f64 * p[2] + self.cache_read as f64 * p[3]) / 1e6
    }
}

pub enum Progress {
    Step(String),
    // `preset`: a preset the result started from, whose range and camera come with it.
    Done { graph: Box<Graph>, summary: String, usage: Usage, changed: bool, preset: Option<&'static presets::Preset> },
    Failed { error: String, usage: Usage },
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        // A turn can think for minutes.
        .timeout_recv_response(Some(std::time::Duration::from_secs(600)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .user_agent(format!("ProceduralFxStudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

// One model call, in the Messages shape whichever backend runs it.
fn call(agent: &ureq::Agent, backend: &Backend, body: &Value, cancel: &AtomicBool) -> Result<Value, String> {
    match backend {
        Backend::Credits { server, token, .. } => call_messages(agent, &format!("{server}/v1/messages"), Auth::Session(token), body, cancel),
        Backend::Anthropic { key, model } => call_messages(agent, API_URL, if model.starts_with("claude-haiku") { Auth::Key(key) } else { Auth::KeyWithFallback(key) }, body, cancel),
        Backend::Compatible { base_url, key, vision, .. } => providers::call(agent, base_url, key, body, *vision, cancel),
    }
}

enum Auth<'a> {
    Key(&'a str),
    KeyWithFallback(&'a str),
    Session(&'a str),
}

// One Messages API call (to Anthropic, or the credits server). Retries dropped
// connections, rate limits and overloads.
fn call_messages(agent: &ureq::Agent, url: &str, auth: Auth, body: &Value, cancel: &AtomicBool) -> Result<Value, String> {
    let mut wait = 2u64;
    for attempt in 0..5 {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let mut request = agent.post(url).header("content-type", "application/json");
        request = match auth {
            Auth::Session(token) => request.header("authorization", &format!("Bearer {token}")),
            Auth::Key(key) => request.header("x-api-key", key).header("anthropic-version", "2023-06-01"),
            Auth::KeyWithFallback(key) => request.header("x-api-key", key).header("anthropic-version", "2023-06-01").header("anthropic-beta", FALLBACK_BETA),
        };
        match request.send_json(body) {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let text = response.body_mut().read_to_string().map_err(|e| format!("Couldn't read Claude's reply: {e}"))?;
                if status == 200 {
                    return serde_json::from_str(&text).map_err(|e| format!("Unexpected reply from Claude: {e}"));
                }
                let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_owned)).unwrap_or(text);
                let session = matches!(auth, Auth::Session(_));
                match status {
                    401 if session => return Err(message),
                    401 => return Err("The API key was rejected: check it in the assistant's settings".into()),
                    402 => return Err(message),
                    429 | 500 | 502 | 503 | 504 | 529 if attempt < 4 => {}
                    _ => return Err(format!("Claude error {status}: {message}")),
                }
            }
            // A dropped connection (e.g. closed without TLS close_notify): try again.
            Err(e) if attempt < 4 => {
                let _ = e;
            }
            Err(e) => return Err(format!("Couldn't reach {}: {e}", url.split('/').take(3).collect::<Vec<_>>().join("/"))),
        }
        std::thread::sleep(std::time::Duration::from_secs(wait));
        wait *= 2;
    }
    Err("The Claude API is busy: try again in a minute".into())
}

// The geometry view_scene shows: one object or node (in world space), or every object.
fn view_geometry(graph: &Graph, ids: &std::collections::BTreeMap<String, NodeId>, node: Option<&str>, time: f32) -> Result<Mesh, String> {
    let mesh = match node {
        Some(name) => {
            let lookup = ids.get(name).map_or_else(|| name.to_owned(), |id| format!("#{id}"));
            let id = graph.nodes.iter().find(|(id, n)| lookup == format!("#{id}") || n.title(**id) == lookup).map(|(id, _)| *id).ok_or_else(|| format!("no node named '{name}'"))?;
            match graph.nodes[&id].kind {
                NodeKind::Geometry { .. } => graph.cook(id, time, &mut HashMap::new()),
                _ => {
                    let local = graph.cook(id, time, &mut HashMap::new());
                    match graph.enclosing_geometry(id) {
                        Some(object) => graph.xform_at(object, time).apply(local),
                        None => local,
                    }
                }
            }
        }
        None => graph.geometry_ids().into_iter().filter(|g| !graph.nodes[g].bypass).fold(Mesh::default(), |all, g| node_merge(all, graph.cook(g, time, &mut HashMap::new()))),
    };
    Ok(if packed::is_packed(&mesh) { packed::unpack(&mesh, true) } else { mesh })
}

// Runs one tool call; `ids` are the names the model gave new nodes earlier in this request.
// Returns the tool result's content (text, or an image and a caption), whether it's an error,
// and whether it changed the scene.
fn run_tool(graph: &mut Graph, ids: &mut std::collections::BTreeMap<String, NodeId>, name: &str, input: &Value, time: f32) -> (Value, bool, bool) {
    if name == "view_scene" {
        let at = input["frame"].as_f64().map_or(time, |f| ((f as f32) - 1.0).max(0.0) / sims::SIM_FPS);
        let mut view = snapshot::View::named(input["view"].as_str().unwrap_or("perspective")).unwrap_or(snapshot::View { azimuth: 35.0, elevation: 25.0, outlines: false });
        if let Some(a) = input["azimuth"].as_f64() {
            view.azimuth = a as f32;
        }
        if let Some(e) = input["elevation"].as_f64() {
            view.elevation = (e as f32).clamp(-89.0, 89.0);
        }
        view.outlines = input["outlines"].as_bool().unwrap_or(false);
        let shot = view_geometry(graph, ids, input["node"].as_str(), at).and_then(|mesh| snapshot::render(&mesh, &view, 800, 600));
        return match shot {
            Ok(png) => (
                json!([
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": snapshot::base64(&png)}},
                    {"type": "text", "text": format!("Frame {}, azimuth {:.0}°, elevation {:.0}°.", (at * sims::SIM_FPS + 1.0).round(), view.azimuth, view.elevation)}
                ]),
                false,
                false,
            ),
            Err(e) => (json!(e), true, false),
        };
    }
    let (text, is_error, changed) = run_text_tool(graph, ids, name, input, time);
    (json!(text), is_error, changed)
}

fn run_text_tool(graph: &mut Graph, ids: &mut std::collections::BTreeMap<String, NodeId>, name: &str, input: &Value, time: f32) -> (String, bool, bool) {
    // (result text, is_error, changed the scene)
    match name {
        "edit_scene" => match ai::parse_edits(&input["edits"]) {
            Ok(edits) => {
                let report = ai::apply_session(graph, &edits, ids);
                let failed = report.applied == 0 && !report.errors.is_empty();
                (report.to_text(graph), failed, report.applied > 0)
            }
            Err(e) => (format!("The edits couldn't be read: {e}"), true, false),
        },
        "inspect_node" => match input["node"].as_str().map(|n| {
            // Frames count from 1 at time 0.
            let at = input["frame"].as_f64().map_or(time, |f| ((f as f32) - 1.0).max(0.0) / sims::SIM_FPS);
            ai::inspect(graph, ids.get(n).map_or_else(|| n.to_owned(), |id| format!("#{id}")).as_str(), at).map(|t| format!("At frame {}: {t}", (at * sims::SIM_FPS + 1.0).round()))
        }) {
            Some(Ok(text)) => (text, false, false),
            Some(Err(e)) => (e, true, false),
            None => ("give the node to inspect".into(), true, false),
        },
        "describe_scene" => (ai::describe(graph), false, false),
        "load_preset" => match input["name"].as_str().and_then(presets::find) {
            Some(preset) => {
                let ids = graph.merge(&Graph::deserialize(preset.scene));
                let added: Vec<String> = ids.values().filter(|id| graph.nodes.get(id).is_some_and(|n| n.parent.is_none())).map(|id| format!("{} (#{id})", graph.nodes[id].title(*id))).collect();
                let range = format!("{:.2}–{:.2} s (frames {}–{})", preset.range.0, preset.range.1, (preset.range.0 * sims::SIM_FPS).round() as i32 + 1, (preset.range.1 * sims::SIM_FPS).round() as i32 + 1);
                (format!("Added the {} preset: {}. It's made to play over {range}{}.\n\n{}", preset.name, added.join(", "), if preset.looping { ", as a loop" } else { "" }, ai::describe(graph)), false, true)
            }
            None => (format!("no preset named {}; the presets are {}", input["name"], presets::PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")), true, false),
        },
        other => (format!("there's no tool named {other}"), true, false),
    }
}

// A short line for the progress log.
fn step_label(name: &str, input: &Value) -> String {
    match name {
        "edit_scene" => {
            let edits = input["edits"].as_array().map_or(0, |e| e.len());
            let nodes: Vec<String> = input["edits"].as_array().into_iter().flatten().filter(|e| e["op"] == "add_node").filter_map(|e| e["type"].as_str().map(str::to_owned)).collect();
            if nodes.is_empty() { format!("Editing ({edits} change{})", if edits == 1 { "" } else { "s" }) } else { format!("Adding {}", nodes.join(", ")) }
        }
        "inspect_node" => format!("Checking {}", input["node"].as_str().unwrap_or("a node")),
        "describe_scene" => "Reading the scene".into(),
        "load_preset" => format!("Starting from the {} preset", input["name"].as_str().unwrap_or("")),
        "view_scene" => format!("Looking at {}", input["node"].as_str().unwrap_or("the scene")),
        other => other.to_owned(),
    }
}

pub struct Job {
    pub prompt: String,
    pub graph: Graph,
    pub backend: Backend,
    pub time: f32,
}

// The whole exchange, on a worker thread: ask, run the tools, repeat until Claude is done.
pub fn run(job: Job, tx: Sender<Progress>, cancel: Arc<AtomicBool>) {
    let Job { prompt, mut graph, backend, time } = job;
    let model = backend.model().to_owned();
    let claude = !matches!(backend, Backend::Compatible { .. });
    let agent = agent();
    let mut usage = Usage::default();
    let mut changed = false;
    let mut preset = None;
    let mut ids = std::collections::BTreeMap::new();
    let mut messages = vec![json!({
        "role": "user",
        "content": format!("The scene now:\n{}\n\nRequest: {prompt}", ai::describe(&graph)),
    })];
    let system = json!([{"type": "text", "text": system_prompt(), "cache_control": {"type": "ephemeral"}}]);
    let tools = tools();
    for _ in 0..MAX_TURNS {
        let mut body = json!({
            "model": model,
            "max_tokens": 16000,
            "system": system,
            "tools": tools,
            "tool_choice": {"type": "auto"},
            "output_config": {"effort": "high"},
            // The conversation so far is cached too, so each turn only pays for what's new.
            "cache_control": {"type": "ephemeral"},
            "messages": messages,
        });
        if claude && !model.starts_with("claude-haiku") {
            body["fallbacks"] = json!("default");
        }
        let response = match call(&agent, &backend, &body, &cancel) {
            Ok(r) => r,
            Err(error) => {
                let _ = tx.send(Progress::Failed { error, usage });
                return;
            }
        };
        usage.add(&response["usage"]);
        usage.add_billing(&response["pfx_billing"]);
        let content = response["content"].clone();
        let blocks = content.as_array().cloned().unwrap_or_default();
        let text: Vec<String> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str().map(str::to_owned)).collect();
        match response["stop_reason"].as_str().unwrap_or("") {
            "refusal" => {
                let _ = tx.send(Progress::Failed { error: "The model declined this request".into(), usage });
                return;
            }
            "max_tokens" => {
                let _ = tx.send(Progress::Failed { error: "The model's reply was too long and got cut off: try a smaller request, or build it in steps".into(), usage });
                return;
            }
            "tool_use" => {}
            _ => {
                let _ = tx.send(Progress::Done { graph: Box::new(graph), summary: text.join("\n").trim().to_owned(), usage, changed, preset });
                return;
            }
        }
        // The assistant turn goes back unchanged (thinking blocks included).
        messages.push(json!({"role": "assistant", "content": content}));
        let mut results = Vec::new();
        for block in blocks.iter().filter(|b| b["type"] == "tool_use") {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Progress::Failed { error: "Cancelled".into(), usage });
                return;
            }
            let (name, input) = (block["name"].as_str().unwrap_or(""), &block["input"]);
            let _ = tx.send(Progress::Step(step_label(name, input)));
            let (result, is_error, did_change) = run_tool(&mut graph, &mut ids, name, input, time);
            changed |= did_change;
            if name == "load_preset" && !is_error {
                preset = input["name"].as_str().and_then(presets::find);
            }
            results.push(json!({"type": "tool_result", "tool_use_id": block["id"], "content": result, "is_error": is_error}));
        }
        messages.push(json!({"role": "user", "content": results}));
    }
    let _ = tx.send(Progress::Done { graph: Box::new(graph), summary: "Stopped after many steps; here is what was built so far.".into(), usage, changed, preset });
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The credits account: signing in (a code confirmed in the browser), balance, payments
// ---------------------------------------------------------------------------

pub enum AccountMsg {
    // Signing in: the code to confirm, and the page to confirm it on.
    Code { user_code: String, url: String },
    SignedIn { token: String, email: String },
    Account(Value),
    Failed(String),
}

fn open_url(url: &str) {
    let _ = crate::platform::open(url);
}

// A JSON call to the credits server; errors carry the server's message.
fn server_json(server: &str, path: &str, token: Option<&str>, body: Option<&Value>) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent(format!("ProceduralFxStudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let url = format!("{server}{path}");
    let bearer = token.map(|t| format!("Bearer {t}"));
    let sent = match body {
        Some(b) => {
            let mut r = agent.post(&url);
            if let Some(b) = &bearer {
                r = r.header("authorization", b);
            }
            r.send_json(b)
        }
        None => {
            let mut r = agent.get(&url);
            if let Some(b) = &bearer {
                r = r.header("authorization", b);
            }
            r.call()
        }
    };
    let mut response = sent.map_err(|e| format!("Couldn't reach {server}: {e}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status == 200 {
        Ok(v)
    } else {
        Err(v["error"]["message"].as_str().map_or_else(|| format!("{server} answered {status}"), str::to_owned))
    }
}

// Signs in on a worker thread: gets a code, opens the browser to confirm it, then waits.
fn sign_in(server: String, tx: Sender<AccountMsg>, cancel: Arc<AtomicBool>) {
    let start = match server_json(&server, "/v1/device/start", None, Some(&json!({}))) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(AccountMsg::Failed(e));
            return;
        }
    };
    let (device, code, url) = (start["device_code"].as_str().unwrap_or(""), start["user_code"].as_str().unwrap_or(""), start["verification_url"].as_str().unwrap_or(""));
    let _ = tx.send(AccountMsg::Code { user_code: code.into(), url: url.into() });
    open_url(url);
    let interval = std::time::Duration::from_secs(start["interval"].as_u64().unwrap_or(3).max(1));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(start["expires_in"].as_u64().unwrap_or(900));
    let mut misses = 0;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(interval);
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match server_json(&server, "/v1/device/token", None, Some(&json!({"device_code": device}))) {
            Ok(v) if v["status"] == "ok" => {
                let _ = tx.send(AccountMsg::SignedIn { token: v["token"].as_str().unwrap_or("").into(), email: v["email"].as_str().unwrap_or("").into() });
                return;
            }
            Ok(_) => misses = 0,
            // A dropped connection now and then is fine; an expired code isn't.
            Err(e) if e.starts_with("Couldn't reach") && misses < 5 => misses += 1,
            Err(_) => break,
        }
    }
    let _ = tx.send(AccountMsg::Failed("The sign-in code expired: sign in again".into()));
}

pub struct Panel {
    pub open: bool,
    settings: Settings,
    key_input: String,
    show_settings: bool,
    prompt: String,
    log: Vec<String>,
    rx: Option<Receiver<Progress>>,
    cancel: Arc<AtomicBool>,
    // What runs the current (or last) request, for its cost.
    running: Option<Backend>,
    // The finished preview: the edited scene, the model's summary, and whether it changed anything.
    result: Option<(Box<Graph>, String, bool, Option<&'static presets::Preset>)>,
    started_from: u64,
    error: Option<String>,
    usage: Usage,
    last_cost: Option<f64>,
    session_cost: f64,
    // The models the server listed (⚙ → List models), and the listing in progress.
    listed: Vec<String>,
    listing: Option<Receiver<Result<Vec<String>, String>>>,
    // Credits: the account as the server last described it, sign-in in progress, and
    // when a payment page was opened (the balance is refreshed for a while after).
    account: Option<Value>,
    account_tx: Sender<AccountMsg>,
    account_rx: Receiver<AccountMsg>,
    account_fetched: Option<std::time::Instant>,
    signing_in: Option<(String, String)>,
    sign_in_cancel: Arc<AtomicBool>,
    paying_since: Option<std::time::Instant>,
    // The first request's size in tokens, and when it was last measured.
    request_tokens: Option<(f64, std::time::Instant)>,
}

impl Default for Panel {
    fn default() -> Self {
        let settings = Settings::load();
        let (account_tx, account_rx) = std::sync::mpsc::channel();
        Panel {
            open: false,
            key_input: String::new(),
            show_settings: settings.backend().is_err(),
            settings,
            prompt: String::new(),
            log: Vec::new(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            running: None,
            result: None,
            started_from: 0,
            error: None,
            usage: Usage::default(),
            last_cost: None,
            session_cost: 0.0,
            listed: Vec::new(),
            listing: None,
            account: None,
            account_tx,
            account_rx,
            account_fetched: None,
            signing_in: None,
            sign_in_cancel: Arc::new(AtomicBool::new(false)),
            paying_since: None,
            request_tokens: None,
        }
    }
}

// A typical build's cost in list-price US dollars, given the first request's size in
// tokens. The first turn writes the system prompt, tools, scene and request to the cache;
// each later turn reads them back and adds the last turn's output and tool results.
// Most builds take 2 to 6 turns of about 1,500 tokens out each.
pub fn estimate(model: &str, first_tokens: f64) -> (f64, f64) {
    let p = MODELS.iter().find(|(id, ..)| *id == model).map_or(MODELS[0].2, |m| m.2);
    let turns = |n: usize| {
        let (mut cached, mut new, mut cost) = (0.0, first_tokens, 0.0);
        for _ in 0..n {
            cost += new * p[2] + cached * p[3] + 1500.0 * p[1];
            cached += new;
            new = 2000.0;
        }
        cost / 1e6
    };
    (turns(2), turns(6))
}

fn fingerprint(graph: &Graph) -> u64 {
    scene_fingerprint(&graph.serialize())
}

impl Panel {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    fn start(&mut self, graph: &Graph, time: f32) {
        let backend = match self.settings.backend() {
            Ok(b) => b,
            Err(e) => {
                self.show_settings = true;
                self.error = Some(e);
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.rx = Some(rx);
        self.result = None;
        self.error = None;
        self.log = vec!["Thinking…".into()];
        self.started_from = fingerprint(graph);
        self.running = Some(backend.clone());
        let job = Job { prompt: self.prompt.trim().to_owned(), graph: graph.clone(), backend, time };
        let cancel = self.cancel.clone();
        std::thread::spawn(move || run(job, tx, cancel));
    }

    fn finished(&mut self, usage: Usage) {
        if let (Some(balance), Some(account)) = (usage.balance, self.account.as_mut()) {
            account["balance"] = json!(balance);
        }
        if matches!(self.running, Some(Backend::Credits { .. })) {
            self.fetch_account();
        }
        self.usage = usage;
        self.last_cost = self.running.as_ref().and_then(|b| b.cost(&usage));
        self.session_cost += self.last_cost.unwrap_or(0.0);
        self.rx = None;
    }

    // Asks the server for the balance and plans (on a worker thread).
    fn fetch_account(&mut self) {
        let Some(server) = credits_server() else { return };
        if self.settings.token.is_empty() {
            return;
        }
        self.account_fetched = Some(std::time::Instant::now());
        let (tx, token) = (self.account_tx.clone(), self.settings.token.clone());
        std::thread::spawn(move || {
            let _ = tx.send(match server_json(&server, "/v1/account", Some(&token), None) {
                Ok(v) => AccountMsg::Account(v),
                Err(e) => AccountMsg::Failed(e),
            });
        });
    }

    // Opens a Stripe page (a plan, a top-up, or managing the plan) in the browser.
    fn pay(&mut self, path: &'static str, body: Value) {
        let Some(server) = credits_server() else { return };
        let (tx, token) = (self.account_tx.clone(), self.settings.token.clone());
        self.paying_since = Some(std::time::Instant::now());
        std::thread::spawn(move || match server_json(&server, path, Some(&token), Some(&body)) {
            Ok(v) => open_url(v["url"].as_str().unwrap_or("")),
            Err(e) => {
                let _ = tx.send(AccountMsg::Failed(e));
            }
        });
    }

    fn sign_out(&mut self) {
        if let Some(server) = credits_server() {
            let token = std::mem::take(&mut self.settings.token);
            std::thread::spawn(move || {
                let _ = server_json(&server, "/v1/logout", Some(&token), Some(&json!({})));
            });
        }
        self.settings.token.clear();
        self.settings.email.clear();
        self.account = None;
        let _ = self.settings.save();
    }

    fn poll_account(&mut self) {
        while let Ok(msg) = self.account_rx.try_recv() {
            match msg {
                AccountMsg::Code { user_code, url } => self.signing_in = Some((user_code, url)),
                AccountMsg::SignedIn { token, email } => {
                    self.signing_in = None;
                    self.settings.token = token;
                    self.settings.email = email;
                    self.error = None;
                    if let Err(e) = self.settings.save() {
                        self.error = Some(format!("Couldn't save the sign-in: {e}"));
                    }
                    self.fetch_account();
                }
                AccountMsg::Account(v) => self.account = Some(v),
                AccountMsg::Failed(e) => {
                    self.signing_in = None;
                    if e.contains("signed out") {
                        self.settings.token.clear();
                        self.account = None;
                        let _ = self.settings.save();
                    }
                    self.error = Some(e);
                }
            }
        }
        // While a payment page may be open, look for the new credit every few seconds.
        if let Some(since) = self.paying_since {
            if since.elapsed() > std::time::Duration::from_secs(15 * 60) {
                self.paying_since = None;
            } else if self.account_fetched.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5)) {
                self.fetch_account();
            }
        }
        if self.settings.provider == Provider::Credits && !self.settings.token.is_empty() && self.account.is_none() && self.account_fetched.is_none() {
            self.fetch_account();
        }
    }

    // "About A$0.12–0.45 for a typical build", in what the request would be paid with.
    fn estimate_text(&mut self, graph: &Graph) -> Option<String> {
        let (symbol, per_usd) = match self.settings.backend().ok()? {
            Backend::Credits { .. } => (self.symbol(), self.account.as_ref()?["per_usd"].as_f64()?),
            Backend::Anthropic { .. } => ("$".to_owned(), 1.0),
            Backend::Compatible { .. } => return None,
        };
        // Describing the scene isn't free, so it's measured at most once a second.
        if self.request_tokens.is_none_or(|(_, at)| at.elapsed() > std::time::Duration::from_secs(1)) {
            let chars = system_prompt().len() + tools().to_string().len() + ai::describe(graph).len() + self.prompt.len();
            self.request_tokens = Some((chars as f64 / 4.0, std::time::Instant::now()));
        }
        let (low, high) = estimate(&self.settings.model, self.request_tokens?.0);
        Some(format!("About {symbol}{:.2}–{:.2} for a typical build", low * per_usd, high * per_usd))
    }

    // The credits currency symbol (e.g. "A$"), as the server reports it.
    fn symbol(&self) -> String {
        self.account.as_ref().and_then(|a| a["symbol"].as_str()).unwrap_or("$").to_owned()
    }

    fn credits_ui(&mut self, ui: &mut egui::Ui) {
        if self.settings.token.is_empty() {
            ui.label("Sign in with your email to use the assistant without an API key: pay with a monthly plan or top-ups.");
            match self.signing_in.clone() {
                Some((code, url)) => {
                    ui.horizontal(|ui| {
                        ui.label("Finish signing in in your browser. Code:");
                        ui.strong(&code);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Open the page again").clicked() {
                            open_url(&url);
                        }
                        if ui.button("Cancel").clicked() {
                            self.sign_in_cancel.store(true, Ordering::Relaxed);
                            self.signing_in = None;
                        }
                    });
                }
                None => {
                    if ui.button("Sign in…").clicked() {
                        if let Some(server) = credits_server() {
                            self.sign_in_cancel = Arc::new(AtomicBool::new(false));
                            self.signing_in = Some(("…".into(), String::new()));
                            let (tx, cancel) = (self.account_tx.clone(), self.sign_in_cancel.clone());
                            std::thread::spawn(move || sign_in(server, tx, cancel));
                        }
                    }
                }
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label(format!("Signed in as {}", self.settings.email));
            if ui.small_button("Sign out").clicked() {
                self.sign_out();
            }
            if ui.small_button("↻").on_hover_text("Refresh the balance").clicked() {
                self.fetch_account();
            }
        });
        let Some(account) = self.account.clone() else {
            ui.weak("Loading your account…");
            return;
        };
        let money = |k: &str| account[k].as_f64().unwrap_or(0.0);
        let sym = self.symbol();
        ui.horizontal(|ui| {
            ui.label("Credit:");
            ui.strong(format!("{sym}{:.2}", money("balance")));
            if let Some(plan) = account["plan"].as_str() {
                ui.weak(format!("({plan} plan {sym}{:.2} this month, top-ups {sym}{:.2})", money("plan_credit"), money("topup_credit")));
            }
        });
        if account["payments"] != true {
            ui.weak("Payments aren't set up on the server yet.");
            return;
        }
        let plans = account["plans"].as_array().cloned().unwrap_or_default();
        if account["plan"].is_string() {
            if ui.button("Manage or cancel your plan…").clicked() {
                self.pay("/v1/portal", json!({}));
            }
        } else {
            for plan in &plans {
                let (id, credit) = (plan["id"].as_str().unwrap_or(""), plan["credit"].as_f64().unwrap_or(0.0));
                let price = plan["price"].as_str().map_or(String::new(), |p| format!("{p}: "));
                let mut name = id.to_owned();
                if let Some(first) = name.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                if ui.button(format!("{name} plan, {price}{sym}{credit:.0} of credit every month")).clicked() {
                    self.pay("/v1/checkout", json!({"plan": id}));
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Top up:");
            for amount in account["topups"].as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_u64) {
                if ui.button(format!("{sym}{amount}")).clicked() {
                    self.pay("/v1/checkout", json!({"topup": amount}));
                }
            }
        });
        ui.weak("Credit never expires: unused plan credit rolls over to the next month. Payment happens in your browser.");
    }

    fn poll(&mut self) {
        self.poll_account();
        if let Some(rx) = &self.listing {
            if let Ok(listed) = rx.try_recv() {
                self.listing = None;
                match listed {
                    Ok(models) => {
                        if self.settings.compat_model.trim().is_empty() || !models.contains(&self.settings.compat_model) {
                            self.settings.compat_model = models[0].clone();
                            let _ = self.settings.save();
                        }
                        self.listed = models;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        let Some(rx) = &self.rx else { return };
        while let Ok(progress) = rx.try_recv() {
            match progress {
                Progress::Step(step) => self.log.push(step),
                Progress::Done { graph, summary, usage, changed, preset } => {
                    self.result = Some((graph, if summary.is_empty() { "Done.".into() } else { summary }, changed, preset));
                    self.finished(usage);
                    return;
                }
                Progress::Failed { error, usage } => {
                    self.error = Some(error);
                    self.finished(usage);
                    return;
                }
            }
        }
    }

    fn key_field(&mut self, ui: &mut egui::Ui, saved: bool, hint: &str, compat: bool) {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.key_input).password(true).hint_text(if saved { "saved (enter a new one to replace it)" } else { hint }).desired_width(260.0));
            if ui.add_enabled(!self.key_input.trim().is_empty(), egui::Button::new("Save")).clicked() {
                let key = std::mem::take(&mut self.key_input).trim().to_owned();
                if compat {
                    self.settings.set_compat_key(key);
                } else {
                    self.settings.api_key = key;
                }
                if let Err(e) = self.settings.save() {
                    self.error = Some(format!("Couldn't save the key: {e}"));
                }
            }
        });
        if saved && ui.small_button("Forget the saved key").clicked() {
            if compat {
                self.settings.set_compat_key(String::new());
            } else {
                self.settings.api_key.clear();
            }
            let _ = self.settings.save();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            let before = self.settings.provider;
            if credits_server().is_some() {
                ui.radio_value(&mut self.settings.provider, Provider::Credits, "ProceduralFX credits (no API key needed)");
            }
            ui.radio_value(&mut self.settings.provider, Provider::Anthropic, "Your own Claude API key");
            ui.radio_value(&mut self.settings.provider, Provider::Compatible, "Another service, or a model on your own computer");
            if self.settings.provider != before {
                self.key_input.clear();
                let _ = self.settings.save();
            }
            ui.add_space(2.0);
            match self.settings.provider {
                Provider::Credits => self.credits_ui(ui),
                Provider::Anthropic => {
                    ui.label("Your Anthropic API key (from console.anthropic.com → API Keys). It's stored on this computer only, readable only by you, and used only to call Claude for your requests.");
                    let saved = !self.settings.api_key.is_empty();
                    self.key_field(ui, saved, "sk-ant-…", false);
                }
                Provider::Compatible => {
                    ui.label("Any server that speaks the OpenAI chat completions format: another company's service, or a model running on your own computer or network.");
                    ui.horizontal(|ui| {
                        ui.label("Service");
                        egui::ComboBox::from_id_salt("assistant_preset").selected_text(providers::preset_label(&self.settings.base_url)).width(230.0).show_ui(ui, |ui| {
                            for (label, url, _) in providers::PRESETS {
                                if ui.selectable_label(self.settings.base_url.trim_end_matches('/') == url, label).clicked() {
                                    self.settings.base_url = url.into();
                                    self.listed.clear();
                                    let _ = self.settings.save();
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Address");
                        if ui.add(egui::TextEdit::singleline(&mut self.settings.base_url).hint_text("http://localhost:11434/v1").desired_width(290.0)).lost_focus() {
                            self.listed.clear();
                            let _ = self.settings.save();
                        }
                    });
                    let saved = !self.settings.compat_key().is_empty();
                    ui.label(if providers::is_local(&self.settings.base_url) { "API key (usually not needed on this computer)" } else { "API key for this service" });
                    self.key_field(ui, saved, "key", true);
                    ui.horizontal(|ui| {
                        ui.label("Model");
                        if self.listed.is_empty() {
                            if ui.add(egui::TextEdit::singleline(&mut self.settings.compat_model).hint_text("e.g. qwen3:32b").desired_width(200.0)).lost_focus() {
                                let _ = self.settings.save();
                            }
                        } else {
                            egui::ComboBox::from_id_salt("assistant_compat_model").selected_text(self.settings.compat_model.clone()).width(220.0).show_ui(ui, |ui| {
                                for m in self.listed.clone() {
                                    if ui.selectable_label(self.settings.compat_model == m, &m).clicked() {
                                        self.settings.compat_model = m;
                                        let _ = self.settings.save();
                                    }
                                }
                            });
                        }
                        let listing = self.listing.is_some();
                        if ui.add_enabled(!listing, egui::Button::new(if listing { "Listing…" } else { "List models" })).clicked() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let (url, key) = (self.settings.base_url.clone(), self.settings.compat_key());
                            std::thread::spawn(move || {
                                let _ = tx.send(providers::list_models(&url, &key));
                            });
                            self.listing = Some(rx);
                        }
                    });
                    if ui.checkbox(&mut self.settings.vision, "The model can see images").on_hover_text("Lets the model look at the scene (view_scene). Turn off for text-only models.").changed() {
                        let _ = self.settings.save();
                    }
                    ui.weak("Pick a model that supports tool calling; bigger models build better setups. For Ollama, raise the context length (e.g. OLLAMA_CONTEXT_LENGTH=32768): the node catalog alone is several thousand tokens.");
                }
            }
        });
    }

    // Shows the panel; returns a scene to switch to when the user accepts a result.
    pub fn show(&mut self, ctx: &egui::Context, graph: &Graph, time: f32) -> Option<(Graph, Option<&'static presets::Preset>)> {
        self.poll();
        if self.busy() || self.listing.is_some() || self.signing_in.is_some() || self.paying_since.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if !self.open {
            return None;
        }
        let mut accepted = None;
        let mut open = self.open;
        egui::Window::new("✨ AI Assistant").open(&mut open).default_width(420.0).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Model");
                match self.settings.provider {
                    Provider::Credits | Provider::Anthropic => {
                        let current = MODELS.iter().find(|(id, ..)| *id == self.settings.model).map_or(MODELS[0].1, |m| m.1);
                        egui::ComboBox::from_id_salt("assistant_model").selected_text(current).show_ui(ui, |ui| {
                            for (id, label, _) in MODELS {
                                if ui.selectable_label(self.settings.model == id, label).clicked() {
                                    self.settings.model = id.into();
                                    let _ = self.settings.save();
                                }
                            }
                        });
                        if let Some(balance) = self.account.as_ref().filter(|_| self.settings.provider == Provider::Credits).and_then(|a| a["balance"].as_f64()) {
                            ui.weak(format!("{}{balance:.2} left", self.symbol()));
                        }
                    }
                    Provider::Compatible => {
                        let model = if self.settings.compat_model.trim().is_empty() { "(none chosen)" } else { self.settings.compat_model.trim() };
                        ui.strong(format!("{model} · {}", providers::preset_label(&self.settings.base_url)));
                    }
                }
                if ui.small_button("⚙").on_hover_text("Model, account and API keys").clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
            if self.show_settings {
                self.settings_ui(ui);
            }
            ui.add_space(4.0);
            ui.label("Describe what you want, or a change to the current scene:");
            ui.add_enabled(
                !self.busy(),
                egui::TextEdit::multiline(&mut self.prompt).hint_text("e.g. a grid of 200 rocks of random sizes scattered on a hilly terrain").desired_rows(3).desired_width(f32::INFINITY),
            );
            let ready = self.settings.backend();
            ui.horizontal(|ui| {
                let can_start = !self.busy() && !self.prompt.trim().is_empty() && ready.is_ok();
                if ui.add_enabled(can_start, egui::Button::new("✨ Build")).clicked() {
                    self.start(graph, time);
                }
                if self.busy() && ui.button("Cancel").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                if let Err(missing) = &ready {
                    ui.weak(missing);
                }
            });
            if let Some(text) = self.estimate_text(graph) {
                ui.weak(text).on_hover_text(
                    "Before you build: what a typical request costs with this model and this scene. \
                     Most take 2 to 6 steps; a long one can cost more. What each request actually cost is shown when it finishes.",
                );
            }
            if !self.log.is_empty() {
                ui.separator();
                for line in &self.log {
                    ui.weak(format!("• {line}"));
                }
                if self.busy() {
                    ui.add(egui::Spinner::new());
                }
            }
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(240, 110, 110), error);
            }
            let mut discard = false;
            if let Some((result, summary, changed, preset)) = &self.result {
                ui.separator();
                ui.label(summary);
                if *changed {
                    let moved_on = fingerprint(graph) != self.started_from;
                    if moved_on {
                        ui.colored_label(egui::Color32::from_rgb(240, 190, 90), "The scene changed since this started: accepting replaces those changes.");
                    }
                    ui.horizontal(|ui| {
                        if ui.button("✔ Accept").on_hover_text("Use this scene (Ctrl+Z undoes it in one step)").clicked() {
                            accepted = Some(((**result).clone(), *preset));
                        }
                        if ui.button("✖ Discard").clicked() {
                            discard = true;
                        }
                    });
                }
            }
            if accepted.is_some() || discard {
                self.result = None;
                self.log.clear();
            }
            if self.usage.input + self.usage.output > 0 {
                ui.separator();
                let tokens = format!(
                    "Last request: {} tokens in ({} cached), {} out",
                    self.usage.input + self.usage.cache_write + self.usage.cache_read,
                    self.usage.cache_read,
                    self.usage.output
                );
                let charged = matches!(self.running, Some(Backend::Credits { .. }));
                ui.weak(match self.last_cost {
                    Some(cost) if charged => format!("{tokens}, charged {sym}{cost:.3}. This session: {sym}{:.2}.", self.session_cost, sym = self.symbol()),
                    Some(cost) => format!("{tokens}, about ${cost:.3}. This session: about ${:.2}.", self.session_cost),
                    None => format!("{tokens}."),
                });
            }
        });
        self.open = open;
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_rise_with_the_model_and_the_scene() {
        let graph = Graph::default();
        let tokens = (system_prompt().len() + tools().to_string().len() + ai::describe(&graph).len()) as f64 / 4.0;
        let (low, high) = estimate("claude-opus-5-5", tokens);
        println!("{tokens:.0} tokens: Opus ${low:.3}–{high:.3}, Haiku ${:.4}–{:.4}", estimate("claude-haiku-5-5", tokens).0, estimate("claude-haiku-5-5", tokens).1);
        assert!(0.0 < low && low < high);
        assert!(estimate("claude-haiku-5-5", tokens).1 < estimate("claude-sonnet-5-5", tokens).0);
        assert!(estimate("claude-opus-5-5", tokens * 2.0).0 > low);
    }

    #[test]
    fn the_prompt_and_tools_are_well_formed() {
        let prompt = system_prompt();
        assert!(prompt.contains("# Geometry node catalog") && prompt.contains("- scatter: "));
        let tools = tools();
        let names: Vec<&str> = tools.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["edit_scene", "inspect_node", "view_scene", "load_preset", "describe_scene"]);
        // The ops the tool offers are the ones ai.rs understands.
        let ops = tools[0]["input_schema"]["properties"]["edits"]["items"]["properties"]["op"]["enum"].as_array().unwrap();
        for op in ops {
            let sample = json!([{"op": op, "id": "x", "type": "box", "object": "o", "node": "n", "from": "a", "to": "b", "input": 1}]);
            let err = ai::parse_edits(&sample).err().unwrap_or_default();
            assert!(!err.contains("unknown variant"), "{op}: {err}");
        }
    }

    #[test]
    fn presets_load_beside_the_scene_and_their_channels_can_be_set() {
        let mut g = Graph::default();
        let mut ids = std::collections::BTreeMap::new();
        let (_, is_error, _) = run_tool(&mut g, &mut ids, "edit_scene", &json!({"edits": [{"op": "create_object", "id": "obj"}, {"op": "add_node", "id": "b", "type": "box", "object": "obj"}]}), 0.0);
        assert!(!is_error);
        let (text, is_error, changed) = run_tool(&mut g, &mut ids, "load_preset", &json!({"name": "impact"}), 0.0);
        let text = text.as_str().unwrap().to_owned();
        assert!(!is_error && changed, "{text}");
        // The box is still there, the preset's object and camera are added, and its settings show.
        assert!(text.contains("Added the Impact preset") && text.contains("sheet_camera") && text.contains("play over 0.00–0.50 s"), "{text}");
        assert!(text.contains("box") && text.contains("channels: count=70 life=0.45 size=0.03 speed=9"), "{text}");
        // Its channels are set by name, like parameters.
        let (text, is_error, _) = run_tool(&mut g, &mut ids, "edit_scene", &json!({"edits": [{"op": "set_params", "node": "impact_points", "params": {"count": 20}}]}), 0.0);
        assert!(!is_error, "{text}");
        let points = g.nodes.iter().find(|(_, n)| n.name == "impact_points").map(|(id, _)| *id).unwrap();
        assert_eq!(g.nodes[&points].channels["count"], [20.0, 0.0, 0.0]);
        let (text, is_error, _) = run_tool(&mut g, &mut ids, "load_preset", &json!({"name": "rain"}), 0.0);
        assert!(is_error && text.as_str().unwrap().contains("Fire, Smoke, Impact, Magic burst, Slash"), "{text}");
        assert!(step_label("load_preset", &json!({"name": "Slash"})).contains("Slash"));
        assert!(system_prompt().contains("Magic burst (plays once)"));
    }

    #[test]
    fn tools_run_against_a_copy_of_the_scene() {
        let mut g = Graph::default();
        let edits = json!({"edits": [
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "b", "type": "box", "object": "obj"},
            {"op": "set_display", "node": "b"}
        ]});
        let mut ids = std::collections::BTreeMap::new();
        let (text, is_error, changed) = run_tool(&mut g, &mut ids, "edit_scene", &edits, 0.0);
        assert!(!is_error && changed, "{text}");
        let (text, is_error, _) = run_tool(&mut g, &mut ids, "inspect_node", &json!({"node": "b"}), 0.0);
        assert!(!is_error && text.as_str().unwrap().contains("6 primitives"), "{text}");
        let (text, is_error, _) = run_tool(&mut g, &mut ids, "edit_scene", &json!({"edits": [{"op": "fly"}]}), 0.0);
        assert!(is_error && text.as_str().unwrap().contains("couldn't be read"), "{text}");
        // The view comes back as a PNG image and a caption.
        let (content, is_error, _) = run_tool(&mut g, &mut ids, "view_scene", &json!({"view": "top", "frame": 1}), 0.0);
        assert!(!is_error && content[0]["type"] == "image" && content[0]["source"]["media_type"] == "image/png", "{content}");
        assert!(content[1]["text"].as_str().unwrap().contains("elevation 89"));
        let (content, is_error, _) = run_tool(&mut g, &mut ids, "view_scene", &json!({"node": "nosuch"}), 0.0);
        assert!(is_error && content.as_str().unwrap().contains("no node"), "{content}");
        assert!(step_label("edit_scene", &edits).contains("box"));
        // Costs at list prices: 1M cached reads on Opus 5.5 = $0.20.
        let usage = Usage { cache_read: 1_000_000, ..Usage::default() };
        assert!((usage.cost("claude-opus-5-5") - 0.20).abs() < 1e-9);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    // Calls the real API with the key saved in the assistant's settings (costs a few cents):
    // `cargo test -- --ignored assistant_builds_a_scene_live --nocapture`.
    #[test]
    #[ignore]
    fn assistant_builds_a_scene_live() {
        let settings = Settings::load();
        let backend = settings.backend().expect("set up a model in the assistant panel first");
        let (tx, rx) = std::sync::mpsc::channel();
        let job = Job {
            // PFX_AI_PROMPT tries another request.
            prompt: std::env::var("PFX_AI_PROMPT").unwrap_or_else(|_| "Scatter 40 small spheres on a 6 by 6 grid, each lifted so it sits on the grid.".into()),
            // PFX_AI_SCENE starts from a saved scene (to test changes to an existing setup).
            graph: std::env::var("PFX_AI_SCENE").ok().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Graph::deserialize(&t)).unwrap_or_default(),
            backend: backend.clone(),
            time: 0.0,
        };
        run(job, tx, Arc::new(AtomicBool::new(false)));
        for progress in rx.iter() {
            match progress {
                Progress::Step(s) => println!("step: {s}"),
                Progress::Done { graph, summary, usage, changed, .. } => {
                    println!("summary: {summary}\n{}", ai::describe(&graph));
                    println!("usage: {usage:?}, about ${:.3}", backend.cost(&usage).unwrap_or(0.0));
                    // PFX_AI_SAVE keeps the result (e.g. to open it in the app).
                    if let Ok(file) = std::env::var("PFX_AI_SAVE") {
                        std::fs::write(file, graph.serialize()).unwrap();
                    }
                    assert!(changed);
                    let object = graph.nodes.iter().find(|(_, n)| matches!(n.kind, NodeKind::Geometry { .. })).map(|(id, _)| *id).expect("an object");
                    let shown = graph.display_of(object).map(|d| graph.cook(d, 0.0, &mut HashMap::new())).unwrap_or_default();
                    assert!(!shown.vertices.is_empty(), "the object shows geometry");
                }
                Progress::Failed { error, usage } => panic!("{error} ({usage:?})"),
            }
        }
    }
}

#[cfg(test)]
mod frames {
    use super::*;

    #[test]
    fn inspect_takes_a_frame() {
        let mut g = Graph::default();
        let mut ids = std::collections::BTreeMap::new();
        run_tool(&mut g, &mut ids, "edit_scene", &json!({"edits": [{"op": "create_object", "id": "o"}, {"op": "add_node", "id": "bx", "type": "box", "object": "o"}]}), 0.0);
        let (text, is_error, _) = run_tool(&mut g, &mut ids, "inspect_node", &json!({"node": "bx", "frame": 49}), 0.0);
        assert!(!is_error && text.as_str().unwrap().starts_with("At frame 49:"), "{text}");
    }
}

#[cfg(test)]
mod inspect_saved {
    use super::*;

    // Inspects a saved scene's node at frames, as the assistant would:
    // `PFX_SCENE=… PFX_NODE=OUT PFX_FRAMES=1,24,72 cargo test -- --ignored inspect_saved_scene --nocapture`.
    #[test]
    #[ignore]
    fn inspect_saved_scene() {
        let g = Graph::deserialize(&std::fs::read_to_string(std::env::var("PFX_SCENE").unwrap()).unwrap());
        let node = std::env::var("PFX_NODE").unwrap_or_else(|_| "OUT".into());
        let mut ids = std::collections::BTreeMap::new();
        let mut g = g;
        // PFX_VIEW_OUT=<file.png>: what view_scene shows Claude (PFX_VIEW names the view).
        if let Ok(out) = std::env::var("PFX_VIEW_OUT") {
            let view = snapshot::View::named(&std::env::var("PFX_VIEW").unwrap_or_else(|_| "perspective".into())).unwrap();
            let frame: f32 = std::env::var("PFX_FRAMES").ok().and_then(|f| f.split(',').next().and_then(|v| v.trim().parse().ok())).unwrap_or(1.0);
            let mesh = view_geometry(&g, &ids, None, (frame - 1.0) / sims::SIM_FPS).unwrap();
            std::fs::write(out, snapshot::render(&mesh, &view, 800, 600).unwrap()).unwrap();
            return;
        }
        for frame in std::env::var("PFX_FRAMES").unwrap_or_else(|_| "1".into()).split(',') {
            let (text, _, _) = run_tool(&mut g, &mut ids, "inspect_node", &json!({"node": node, "frame": frame.trim().parse::<f64>().unwrap()}), 0.0);
            println!("{}", text.as_str().unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod compatible {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    // A stand-in for a local model server: answers each chat completions request with the
    // next canned reply, and keeps the requests it got.
    fn fake_server(replies: Vec<Value>) -> (String, std::thread::JoinHandle<Vec<Value>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/v1", listener.local_addr().unwrap().port());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let mut head = String::new();
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let (mut length, mut line) = (0, String::new());
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut request: Value = serde_json::from_slice(&body).unwrap();
                request["_head"] = json!(head.to_ascii_lowercase());
                seen.push(request);
                let text = reply.to_string();
                let mut stream = reader.into_inner();
                write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len()).unwrap();
            }
            seen
        });
        (url, handle)
    }

    #[test]
    fn builds_a_scene_with_a_local_model() {
        let edits = json!({"edits": [
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "b", "type": "box", "object": "obj"},
            {"op": "set_display", "node": "b"}
        ]});
        let (url, server) = fake_server(vec![
            json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "edit_scene", "arguments": edits.to_string()}}
            ]}}], "usage": {"prompt_tokens": 900, "completion_tokens": 40}}),
            json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "Made a box."}}], "usage": {"prompt_tokens": 950, "completion_tokens": 5}}),
        ]);
        let (tx, rx) = std::sync::mpsc::channel();
        let backend = Backend::Compatible { base_url: url, key: String::new(), model: "local-model".into(), vision: false };
        run(Job { prompt: "a box".into(), graph: Graph::default(), backend: backend.clone(), time: 0.0 }, tx, Arc::new(AtomicBool::new(false)));
        let Some(Progress::Done { graph, summary, usage, changed, .. }) = rx.iter().find(|p| !matches!(p, Progress::Step(_))) else { panic!("didn't finish") };
        assert!(changed && summary == "Made a box.", "{summary}");
        assert!(ai::describe(&graph).contains("box"));
        assert_eq!((usage.input, usage.output), (1850, 45));
        assert_eq!(backend.cost(&usage), None);
        let requests = server.join().unwrap();
        // No Anthropic-only fields, the system prompt first, no view_scene for a text-only model.
        assert_eq!(requests[0]["model"], "local-model");
        assert!(requests[0].get("output_config").is_none() && requests[0].get("fallbacks").is_none());
        assert_eq!(requests[0]["messages"][0]["role"], "system");
        assert!(requests[0]["tools"].as_array().unwrap().iter().all(|t| t["function"]["name"] != "view_scene"));
        // The edit's result went back as a tool message for call c1.
        let tool = requests[1]["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool["tool_call_id"], "c1");
    }

    #[test]
    fn credits_go_through_the_server_with_the_session() {
        let (url, server) = fake_server(vec![json!({
            "content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn", "model": "claude-opus-5-5",
            "usage": {"input_tokens": 100, "output_tokens": 10},
            "pfx_billing": {"charged": 0.0123, "balance": 4.5, "currency": "aud", "symbol": "A$"}
        })]);
        let server_url = url.trim_end_matches("/v1").to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        let backend = Backend::Credits { server: server_url, token: "pfx_abc".into(), model: "claude-opus-5-5".into() };
        run(Job { prompt: "hi".into(), graph: Graph::default(), backend: backend.clone(), time: 0.0 }, tx, Arc::new(AtomicBool::new(false)));
        let Some(Progress::Done { usage, .. }) = rx.iter().find(|p| !matches!(p, Progress::Step(_))) else { panic!("didn't finish") };
        assert!((usage.charged - 0.0123).abs() < 1e-9 && usage.balance == Some(4.5));
        assert_eq!(backend.cost(&usage), Some(usage.charged));
        let requests = server.join().unwrap();
        let head = requests[0]["_head"].as_str().unwrap();
        assert!(head.starts_with("post /v1/messages ") && head.contains("authorization: bearer pfx_abc") && !head.contains("x-api-key"), "{head}");
        // The server gets the same request the app would send Claude.
        assert_eq!(requests[0]["model"], "claude-opus-5-5");
        assert_eq!(requests[0]["system"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn settings_say_what_is_missing() {
        let mut s = Settings { provider: Provider::Compatible, base_url: "https://api.openai.com/v1".into(), ..Settings::default() };
        assert!(s.backend().unwrap_err().contains("model"));
        s.compat_model = "some-model".into();
        assert!(s.backend().unwrap_err().contains("API key"));
        s.set_compat_key(" sk-test ".into());
        assert!(matches!(s.backend(), Ok(Backend::Compatible { ref key, .. }) if key == "sk-test"));
        // Keys are per server; a local server needs none.
        s.base_url = "http://localhost:11434/v1/".into();
        assert!(matches!(s.backend(), Ok(Backend::Compatible { ref key, ref base_url, .. }) if key.is_empty() && base_url == "http://localhost:11434/v1"));
    }
}
