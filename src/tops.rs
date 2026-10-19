// TOPs: task graphs inside a TOP Network (the TASKS context).
//
// Each TOP node turns its input's work items into its own (Houdini's PDG):
// - Frame Range: one item per frame.
// - Wedge: copies of every input item with a parameter of some node set to
//   values spread between two numbers (render or export variations).
// - Fetch: runs an OUT MoonRay render node or a File Export SOP for each item,
//   at the item's frame and with its wedged parameters.
// - Command: runs a shell command per item.
// - File Pattern: one item per file matching a pattern.
// - Wait for All: one item after all of its input's items.
// - FFmpeg Encode: makes a video from its input's output files.
//
// Generation is pure (this file): items, what each one runs and the file it
// makes are known before anything runs. The app runs them (see TopRun).
// Paths and commands expand $F/$F4 (frame), @index, @wedgeindex, @<attribute>,
// and {input} (the item's input file).

use std::collections::HashMap;

use crate::shading::{f, Param};

pub struct TopOp {
    pub label: &'static str,
    pub help: &'static str,
    pub inputs: usize,
    pub params: &'static [Param],
    pub texts: &'static [&'static str],
}

pub const FRAME_RANGE: usize = 0;
pub const WEDGE: usize = 1;
pub const FETCH: usize = 2;
pub const COMMAND: usize = 3;
pub const FILE_PATTERN: usize = 4;
pub const WAIT_FOR_ALL: usize = 5;
pub const FFMPEG: usize = 6;

pub const OPS: &[TopOp] = &[
    TopOp {
        label: "Frame Range",
        help: "One work item per frame (frame 1 is time 0).",
        inputs: 0,
        params: &[f("start", "Start frame", "", -1e5, 1e5, 1.0), f("end", "End frame", "", -1e5, 1e5, 48.0), f("step", "Step", "", 1.0, 1000.0, 1.0)],
        texts: &[],
    },
    TopOp {
        label: "Wedge",
        help: "Copies each input item once per wedge value, setting one parameter of a node to values from Start to End.",
        inputs: 1,
        params: &[
            f("count", "Wedges", "", 1.0, 1000.0, 3.0),
            f("start", "Start value", "", -1e6, 1e6, 0.0),
            f("end", "End value", "", -1e6, 1e6, 1.0),
            f("node", "Node", "", -1.0, 1e9, -1.0),
            f("param", "Parameter", "", 0.0, 1000.0, 0.0),
        ],
        texts: &["Attribute name"],
    },
    TopOp {
        label: "Fetch",
        help: "Runs a MoonRay render node (OUT) or a File Export node for each work item, at its frame and wedge.",
        inputs: 1,
        params: &[f("node", "Node", "", -1.0, 1e9, -1.0)],
        texts: &["Output file (optional)"],
    },
    TopOp {
        label: "Command",
        help: "Runs a shell command for each work item ($F, @index, @wedgeindex, @attribute and {input} are filled in).",
        inputs: 1,
        params: &[],
        texts: &["Command", "Output file (optional)"],
    },
    TopOp {
        label: "File Pattern",
        help: "One work item per file matching a pattern like /renders/shot.*.exr.",
        inputs: 0,
        params: &[],
        texts: &["Pattern"],
    },
    TopOp {
        label: "Wait for All",
        help: "One work item that waits for all of its input's items (and collects their files).",
        inputs: 1,
        params: &[],
        texts: &[],
    },
    TopOp {
        label: "FFmpeg Encode",
        help: "Makes a video from its input's output files (needs ffmpeg installed).",
        inputs: 1,
        params: &[f("fps", "Frames per second", "", 1.0, 120.0, 24.0)],
        texts: &["Video file"],
    },
];

pub const CATEGORY: &str = "Tasks";

#[derive(Clone, Debug, PartialEq)]
pub struct TopNode {
    pub op: usize,
    pub values: Vec<f32>,
    pub texts: Vec<String>,
}

impl TopNode {
    pub fn new(op: usize) -> TopNode {
        let op = op.min(OPS.len() - 1);
        let def = &OPS[op];
        let mut texts = vec![String::new(); def.texts.len()];
        match op {
            WEDGE => texts[0] = "wedge".into(),
            FETCH => {}
            COMMAND => texts[0] = "echo frame $F item @index".into(),
            FFMPEG => texts[0] = "$HOME/render.mp4".into(),
            _ => {}
        }
        TopNode { op, values: def.params.iter().map(|p| p.default[0]).collect(), texts }
    }

    pub fn def(&self) -> &'static TopOp {
        &OPS[self.op.min(OPS.len() - 1)]
    }

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.op as f32];
        p.extend_from_slice(&self.values);
        p
    }

    pub fn from_params(p: &[f32]) -> TopNode {
        let mut node = TopNode::new(p.first().map_or(0, |o| o.max(0.0) as usize));
        for (slot, v) in node.values.iter_mut().zip(p.iter().skip(1)) {
            *slot = *v;
        }
        node
    }

    pub fn v(&self, key: &str) -> f32 {
        self.def().params.iter().position(|p| p.key == key).and_then(|i| self.values.get(i).copied()).unwrap_or(0.0)
    }

    pub fn set(&mut self, key: &str, value: f32) {
        if let Some(i) = self.def().params.iter().position(|p| p.key == key) {
            self.values[i] = value;
        }
    }

    pub fn text(&self, i: usize) -> &str {
        self.texts.get(i).map_or("", |t| t.trim())
    }

    pub fn label(&self, id: usize) -> String {
        format!("{}{id}", self.def().label.to_lowercase().replace(' ', ""))
    }

    pub fn node(&self) -> Option<usize> {
        let n = self.v("node");
        (n >= 0.0).then_some(n as usize)
    }
}

// What a work item runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    // Nothing to run (generators, Wait for All).
    Nothing,
    // A MoonRay render node, writing `output`.
    Render(usize),
    // A File Export node, writing `output`.
    Export(usize),
    Shell(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkItem {
    pub index: usize,
    pub frame: f32,
    pub attrs: Vec<(String, f32)>,
    // Parameters set while the item runs: (node, parameter index, value).
    pub wedges: Vec<(usize, usize, f32)>,
    pub wedge_index: usize,
    // Files from the input item(s).
    pub inputs: Vec<String>,
    pub output: String,
    pub job: Job,
}

impl WorkItem {
    fn root(index: usize, frame: f32) -> WorkItem {
        WorkItem { index, frame, attrs: Vec::new(), wedges: Vec::new(), wedge_index: 0, inputs: Vec::new(), output: String::new(), job: Job::Nothing }
    }

    // The item continuing from `self` (a downstream node's copy).
    fn child(&self, index: usize) -> WorkItem {
        WorkItem { index, inputs: if self.output.is_empty() { self.inputs.clone() } else { vec![self.output.clone()] }, output: String::new(), job: Job::Nothing, ..self.clone() }
    }

    pub fn time(&self, fps: f32) -> f32 {
        (self.frame - 1.0) / fps
    }

    // Fills in $F, $F2..$F9, @index, @wedgeindex, @<attribute>, {input} and {frame}.
    pub fn expand(&self, text: &str, fps: f32) -> String {
        let mut s = crate::fileio::expand_frame(text, self.time(fps), fps);
        s = s.replace("{input}", &self.inputs.join(" ")).replace("{frame}", &format!("{}", self.frame));
        s = s.replace("@wedgeindex", &self.wedge_index.to_string()).replace("@index", &self.index.to_string());
        // Longest names first, so @wedge doesn't eat @wedge2.
        let mut attrs = self.attrs.clone();
        attrs.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        for (name, value) in attrs {
            let shown = if value.fract() == 0.0 { format!("{value:.0}") } else { format!("{value}") };
            s = s.replace(&format!("@{name}"), &shown);
        }
        if let Ok(home) = std::env::var("HOME") {
            s = s.replace("$HOME", &home);
        }
        s
    }
}

// What Fetch needs to know about its target node.
#[derive(Clone, Debug)]
pub struct FetchTarget {
    pub render: bool,
    // The node's own output file (may contain $F or ####).
    pub output: String,
    // The node's own frames, used when Fetch has no input items.
    pub frames: Vec<f32>,
}

pub const FPS: f32 = 24.0;

fn glob(pattern: &str) -> Vec<String> {
    let path = std::path::Path::new(pattern.trim());
    let (dir, name) = (path.parent().unwrap_or(std::path::Path::new(".")), path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let re = regex::Regex::new(&format!("^{}$", regex::escape(&name).replace("\\*", ".*").replace("\\?", "."))).ok();
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| re.as_ref().is_some_and(|re| re.is_match(&e.file_name().to_string_lossy())))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    files.sort();
    files
}

// The work items of `node`, from its input's (`upstream`, None when unconnected).
pub fn generate(node: &TopNode, upstream: Option<&[WorkItem]>, fetch: impl Fn(usize) -> Option<FetchTarget>) -> Result<Vec<WorkItem>, String> {
    let from_input = |f: &dyn Fn(&WorkItem, usize) -> Result<WorkItem, String>| -> Result<Vec<WorkItem>, String> {
        let up = upstream.ok_or("connect an input")?;
        up.iter().enumerate().map(|(i, item)| f(item, i)).collect()
    };
    match node.op {
        FRAME_RANGE => {
            let (start, end, step) = (node.v("start"), node.v("end"), node.v("step").max(1.0));
            let count = (((end - start) / step).floor().max(0.0) as usize + 1).min(100_000);
            Ok((0..count).map(|i| WorkItem::root(i, start + i as f32 * step)).collect())
        }
        WEDGE => {
            let count = (node.v("count").round().max(1.0) as usize).min(10_000);
            let (a, b) = (node.v("start"), node.v("end"));
            let name = if node.text(0).is_empty() { "wedge".to_owned() } else { node.text(0).to_owned() };
            let target = node.node().map(|n| (n, node.v("param").max(0.0) as usize));
            let base = upstream.map(|u| u.to_vec()).unwrap_or_else(|| vec![WorkItem::root(0, 1.0)]);
            let mut out = Vec::new();
            for item in &base {
                for w in 0..count {
                    let t = if count > 1 { w as f32 / (count - 1) as f32 } else { 0.0 };
                    let value = a + (b - a) * t;
                    let mut next = item.child(out.len());
                    next.attrs.push((name.clone(), value));
                    next.wedge_index = w;
                    if let Some((n, p)) = target {
                        next.wedges.push((n, p, value));
                    }
                    out.push(next);
                }
            }
            Ok(out)
        }
        FETCH => {
            let target_id = node.node().ok_or("choose a node to fetch (a MoonRay render or a File Export)")?;
            let target = fetch(target_id).ok_or("the fetched node must be a MoonRay render node or a File Export node")?;
            let template = if node.text(0).is_empty() { target.output.clone() } else { node.text(0).to_owned() };
            let base = match upstream {
                Some(up) => up.to_vec(),
                None => target.frames.iter().enumerate().map(|(i, f)| WorkItem::root(i, *f)).collect(),
            };
            let several = base.len() > 1;
            Ok(base
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let mut next = item.child(i);
                    let mut output = template.clone();
                    // Several items must not overwrite each other: number the file when the name has no tokens.
                    if several && !output.contains("$F") && !output.contains('#') && !output.contains('@') {
                        let path = std::path::Path::new(&output);
                        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
                        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                        output = path.with_file_name(format!("{stem}.@index{ext}")).to_string_lossy().into_owned();
                    }
                    let frame_number = item.frame.round().max(0.0) as usize;
                    next.output = next.expand(&output.replace("####", &format!("{frame_number:04}")), FPS);
                    next.job = if target.render { Job::Render(target_id) } else { Job::Export(target_id) };
                    next
                })
                .collect())
        }
        COMMAND => {
            if node.text(0).is_empty() {
                return Err("enter a command".into());
            }
            let items = match upstream {
                Some(_) => from_input(&|item, i| Ok(item.child(i)))?,
                None => vec![WorkItem::root(0, 1.0)],
            };
            Ok(items
                .into_iter()
                .map(|mut item| {
                    item.output = item.expand(node.text(1), FPS);
                    item.job = Job::Shell(item.expand(node.text(0), FPS));
                    item
                })
                .collect())
        }
        FILE_PATTERN => {
            if node.text(0).is_empty() {
                return Err("enter a file pattern".into());
            }
            Ok(glob(node.text(0))
                .into_iter()
                .enumerate()
                .map(|(i, file)| {
                    let mut item = WorkItem::root(i, i as f32 + 1.0);
                    item.output = file;
                    item
                })
                .collect())
        }
        WAIT_FOR_ALL => {
            let up = upstream.ok_or("connect an input")?;
            let mut item = WorkItem::root(0, up.first().map_or(1.0, |i| i.frame));
            item.inputs = up.iter().map(|i| if i.output.is_empty() { i.inputs.join(" ") } else { i.output.clone() }).filter(|s| !s.is_empty()).collect();
            Ok(vec![item])
        }
        _ => {
            let up = upstream.ok_or("connect the items whose files make the video")?;
            let mut files: Vec<(f32, String)> = up.iter().map(|i| (i.frame, if i.output.is_empty() { i.inputs.first().cloned().unwrap_or_default() } else { i.output.clone() })).filter(|(_, f)| !f.is_empty()).collect();
            files.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut item = WorkItem::root(0, 1.0);
            item.inputs = files.into_iter().map(|(_, f)| f).collect();
            item.output = item.expand(if node.text(0).is_empty() { "$HOME/render.mp4" } else { node.text(0) }, FPS);
            let fps = node.v("fps").max(1.0);
            // ffmpeg's concat demuxer reads the list from a file written next to the video.
            let list = format!("{}.txt", item.output);
            let entries: String = item.inputs.iter().map(|f| format!("file '{}'\\nduration {}\\n", f.replace('\'', "'\\''"), 1.0 / fps)).collect();
            item.job = Job::Shell(format!(
                "printf '{entries}' > '{list}' && ffmpeg -y -f concat -safe 0 -i '{list}' -r {fps} -pix_fmt yuv420p -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' '{}'",
                item.output
            ));
            Ok(vec![item])
        }
    }
}

// Status of one work item while a TOP network cooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Waiting,
    Running,
    Done,
    Failed,
}

// Per-node results of the last cook: each item's state and log line.
pub type Results = HashMap<usize, Vec<(State, String)>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn target(render: bool) -> impl Fn(usize) -> Option<FetchTarget> {
        move |id| (id == 7).then(|| FetchTarget { render, output: "/out/beauty.$F4.exr".into(), frames: vec![1.0, 2.0] })
    }

    #[test]
    fn frames_wedges_and_fetches_chain() {
        let mut range = TopNode::new(FRAME_RANGE);
        range.set("start", 1.0);
        range.set("end", 3.0);
        let frames = generate(&range, None, target(true)).unwrap();
        assert_eq!(frames.iter().map(|i| i.frame).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);
        let mut wedge = TopNode::new(WEDGE);
        wedge.set("count", 2.0);
        wedge.set("start", 0.5);
        wedge.set("end", 1.5);
        wedge.set("node", 12.0);
        wedge.set("param", 4.0);
        let wedged = generate(&wedge, Some(&frames), target(true)).unwrap();
        assert_eq!(wedged.len(), 6);
        assert_eq!(wedged[1].wedges, vec![(12, 4, 1.5)]);
        assert_eq!(wedged[1].attrs, vec![("wedge".to_owned(), 1.5)]);
        let mut fetch = TopNode::new(FETCH);
        fetch.set("node", 7.0);
        fetch.texts[0] = "/out/shot_w@wedgeindex.$F4.exr".into();
        let renders = generate(&fetch, Some(&wedged), target(true)).unwrap();
        assert_eq!(renders.len(), 6);
        assert_eq!(renders[3].output, "/out/shot_w1.0002.exr");
        assert_eq!(renders[3].job, Job::Render(7));
        // Without input, Fetch uses the node's own frames and file name.
        let own = generate(&TopNode { texts: vec![String::new()], ..fetch.clone() }, None, target(false)).unwrap();
        assert_eq!(own.iter().map(|i| i.output.as_str()).collect::<Vec<_>>(), vec!["/out/beauty.0001.exr", "/out/beauty.0002.exr"]);
        assert_eq!(own[0].job, Job::Export(7));
        assert!(generate(&TopNode::new(FETCH), None, target(true)).is_err());
        // Wait for All gathers files; FFmpeg orders them by frame.
        let all = generate(&TopNode::new(WAIT_FOR_ALL), Some(&renders), target(true)).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].inputs.len(), 6);
        let video = generate(&TopNode::new(FFMPEG), Some(&own), target(true)).unwrap();
        assert!(matches!(&video[0].job, Job::Shell(cmd) if cmd.contains("ffmpeg") && cmd.contains("beauty.0001.exr")));
        // Commands fill in tokens.
        let mut cmd = TopNode::new(COMMAND);
        cmd.texts[0] = "convert {input} -resize 50% /tmp/small.$F4.png # @wedge".into();
        let jobs = generate(&cmd, Some(&renders), target(true)).unwrap();
        assert_eq!(jobs[3].job, Job::Shell("convert /out/shot_w1.0002.exr -resize 50% /tmp/small.0002.png # 1.5".into()));
        for i in 0..OPS.len() {
            let n = TopNode::new(i);
            assert_eq!(TopNode::from_params(&n.to_params()).values, n.values);
        }
    }

    #[test]
    fn file_patterns_list_matching_files() {
        let dir = std::env::temp_dir().join(format!("pfx_tops_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.0001.exr", "a.0002.exr", "b.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let mut pattern = TopNode::new(FILE_PATTERN);
        pattern.texts[0] = dir.join("a.*.exr").to_string_lossy().into_owned();
        let items = generate(&pattern, None, |_| None).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].output.ends_with("a.0001.exr"));
    }
}
