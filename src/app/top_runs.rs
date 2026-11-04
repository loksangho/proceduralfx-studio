// TOP networks and image sequences: cooking work items over wedges and frames.

use crate::*;

impl ProceduralFxStudioApp {
    // Starts cooking a TOP node: its input chain's work items first, then its own.
    pub(crate) fn start_top_cook(&mut self, id: NodeId) {
        if self.top_run.is_some() {
            self.status = "A TOP network is already cooking".into();
            return;
        }
        // The chain from the first node down to `id`.
        let mut chain = vec![id];
        while let Some(src) = self.graph.input_of(*chain.last().unwrap(), 0) {
            if chain.contains(&src) || chain.len() > 64 {
                break;
            }
            chain.push(src);
        }
        chain.reverse();
        let mut items = HashMap::new();
        let mut queue = Vec::new();
        for node in &chain {
            match self.graph.top_items(*node) {
                Ok(list) => {
                    queue.extend((0..list.len()).map(|i| (*node, i)));
                    self.top_results.insert(*node, vec![(tops::State::Waiting, String::new()); list.len()]);
                    items.insert(*node, list);
                }
                Err(e) => {
                    self.status = format!("{}: {e}", self.graph.nodes.get(node).map_or(String::new(), |n| n.title(*node)));
                    return;
                }
            }
        }
        queue.reverse();
        let total = queue.len();
        self.top_run = Some(TopRun { queue, items, current: None, total, failed: 0 });
        self.status = format!("Cooking {total} work items…");
        self.advance_top_run();
    }

    pub(crate) fn set_item_state(&mut self, node: NodeId, index: usize, state: tops::State, log: String) {
        if let Some(slot) = self.top_results.get_mut(&node).and_then(|v| v.get_mut(index)) {
            *slot = (state, log);
        }
    }

    // Runs the TOP cook: finishes the running item when it's done, then starts the next.
    pub(crate) fn advance_top_run(&mut self) {
        if std::mem::take(&mut self.top_cancel_request) {
            if let Some(run) = self.top_run.take() {
                if let Some(cancel) = &self.render_cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                for (node, index) in run.queue {
                    self.set_item_state(node, index, tops::State::Failed, "cancelled".into());
                }
                if let Some(TopJob { node, index, .. }) = run.current {
                    self.set_item_state(node, index, tops::State::Failed, "cancelled".into());
                }
                self.status = "TOP cook cancelled".into();
            }
            return;
        }
        loop {
            let Some(run) = self.top_run.as_mut() else { return };
            // The running item.
            if let Some(job) = run.current.as_ref() {
                let finished = match &job.wait {
                    TopWait::Render => {
                        if self.moonray_rx.is_some() {
                            return;
                        }
                        let ok = self.render_ok.unwrap_or(false);
                        Some((ok, if ok { job.output.clone() } else { self.last_render_summary.clone() }))
                    }
                    TopWait::Shell(rx) => match rx.try_recv() {
                        Ok(result) => Some(match result {
                            Ok(log) => (true, log),
                            Err(log) => (false, log),
                        }),
                        Err(mpsc::TryRecvError::Empty) => return,
                        Err(mpsc::TryRecvError::Disconnected) => Some((false, "the command stopped unexpectedly".into())),
                    },
                };
                let (node, index) = (job.node, job.index);
                let run = self.top_run.as_mut().unwrap();
                run.current = None;
                if let Some((ok, log)) = finished {
                    if !ok {
                        run.failed += 1;
                    }
                    self.set_item_state(node, index, if ok { tops::State::Done } else { tops::State::Failed }, log);
                }
                continue;
            }
            let Some((node, index)) = run.queue.pop() else {
                let (total, failed) = (run.total, run.failed);
                self.top_run = None;
                self.render_at = None;
                self.status = if failed == 0 { format!("TOP cook finished: {total} work items") } else { format!("TOP cook finished: {failed} of {total} work items failed") };
                return;
            };
            let item = run.items[&node][index].clone();
            self.set_item_state(node, index, tops::State::Running, String::new());
            match item.job.clone() {
                tops::Job::Nothing => self.set_item_state(node, index, tops::State::Done, item.output.clone()),
                tops::Job::Export(target) => {
                    let result = self.with_wedges(&item, |app| {
                        let source = app.graph.input_of(target, 0).ok_or("the File Export node has no input")?;
                        let mesh = app.graph.cook_fresh(Some(source), item.time(sims::SIM_FPS));
                        write_geometry_file(&mesh, &item.output).map(|_| item.output.clone())
                    });
                    let run = self.top_run.as_mut().unwrap();
                    if result.is_err() {
                        run.failed += 1;
                    }
                    let (state, log) = match result {
                        Ok(out) => (tops::State::Done, out),
                        Err(e) => (tops::State::Failed, e),
                    };
                    self.set_item_state(node, index, state, log);
                }
                tops::Job::Cache(target) => {
                    let result = self.with_wedges(&item, |app| {
                        let source = app.graph.input_of(target, 0).ok_or("the Cache node has no input")?;
                        let mesh = app.graph.cook_fresh(Some(source), item.time(sims::SIM_FPS));
                        save_mesh_cache(std::path::Path::new(&item.output), &mesh).map(|_| item.output.clone()).map_err(|e| format!("couldn't write {}: {e}", item.output))
                    });
                    let run = self.top_run.as_mut().unwrap();
                    if result.is_err() {
                        run.failed += 1;
                    }
                    let (state, log) = match result {
                        Ok(out) => (tops::State::Done, out),
                        Err(e) => (tops::State::Failed, e),
                    };
                    self.set_item_state(node, index, state, log);
                }
                tops::Job::Render(target) => {
                    let Some(NodeKind::MoonRayRender { executable, arguments, width, height, samples, opts, .. }) = self.graph.nodes.get(&target).map(|n| n.kind.clone()) else {
                        self.set_item_state(node, index, tops::State::Failed, "the render node is gone".into());
                        continue;
                    };
                    if self.moonray_rx.is_some() {
                        // Another render is running: try again next frame.
                        let run = self.top_run.as_mut().unwrap();
                        run.queue.push((node, index));
                        return;
                    }
                    self.render_ok = None;
                    let output = item.output.clone();
                    self.with_wedges(&item, |app| {
                        app.render_opts = opts.clone();
                        app.render_at = Some(item.time(sims::SIM_FPS));
                        // Wedged parameters must not come from the cache of unchanging nodes.
                        let cache = std::mem::take(&mut app.static_cache);
                        app.spawn_moonray_render(executable.clone(), arguments.clone(), output.clone(), width, height, samples);
                        app.static_cache = cache;
                        Ok::<(), String>(())
                    })
                    .ok();
                    if self.moonray_rx.is_none() {
                        let reason = self.status.clone();
                        self.top_run.as_mut().unwrap().failed += 1;
                        self.set_item_state(node, index, tops::State::Failed, reason);
                        continue;
                    }
                    self.status = format!("TOP: rendering {output}");
                    self.top_run.as_mut().unwrap().current = Some(TopJob { node, index, output, wait: TopWait::Render });
                    return;
                }
                tops::Job::Shell(command) => {
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = platform::shell(&command).output();
                        let _ = tx.send(match result {
                            Ok(out) => {
                                let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_owned();
                                if out.status.success() { Ok(log) } else { Err(if log.is_empty() { format!("exit status {}", out.status) } else { log }) }
                            }
                            Err(e) => Err(e.to_string()),
                        });
                    });
                    self.top_run.as_mut().unwrap().current = Some(TopJob { node, index, output: item.output.clone(), wait: TopWait::Shell(rx) });
                    return;
                }
            }
        }
    }

    // Runs `f` with the item's wedged parameters set, putting them back afterwards.
    pub(crate) fn with_wedges<T>(&mut self, item: &tops::WorkItem, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        let mut restore = Vec::new();
        for (node, param, value) in &item.wedges {
            if let Some(old) = self.graph.set_param(*node, *param, *value) {
                restore.push((*node, *param, old));
            }
        }
        let result = f(self);
        for (node, param, old) in restore.into_iter().rev() {
            self.graph.set_param(node, param, old);
        }
        result
    }

    // File Output COP: writes the cooked image (one file per frame over the timeline when asked).
    pub(crate) fn write_cop_output(&mut self, id: NodeId, range: bool) -> Result<String, String> {
        let Some(NodeKind::Cop(cop)) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return Err("not a File Output node".into());
        };
        let file = cop.texts.first().map(|f| f.trim().to_owned()).filter(|f| !f.is_empty()).ok_or("choose a file to write")?;
        let times = if range { frame_times(self.playback_start.max(0.0), self.playback_end, 1.0 / sims::SIM_FPS) } else { vec![self.time] };
        let mut written = Vec::new();
        for time in times {
            let mesh = self.graph.cook_fresh(Some(id), time);
            let img = cops::Image::from_mesh(&mesh).ok_or_else(|| node_errors(|e| e.get(&id).cloned()).unwrap_or_else(|| "no image to write".into()))?;
            let path = fileio::expand_frame(&file, time, sims::SIM_FPS);
            cops::write_image(&img, &path)?;
            written.push(path);
        }
        Ok(match written.as_slice() {
            [one] => format!("Wrote {one}"),
            many => format!("Wrote {} images ({} … {})", many.len(), many[0], many[many.len() - 1]),
        })
    }

    pub(crate) fn begin_sequence(&mut self, job: SequenceJob) {
        if job.times.is_empty() {
            self.status = "Frame range is empty".into();
            return;
        }
        self.sequence = Some(job);
        self.advance_sequence();
    }

    // Starts the next frame of the running sequence once the previous one has finished.
    pub(crate) fn advance_sequence(&mut self) {
        if self.moonray_rx.is_some() {
            return;
        }
        let Some(job) = self.sequence.as_mut() else {
            return;
        };
        let total = job.times.len();
        if job.next >= total {
            let first = PathBuf::from(frame_output(&job.output, 1));
            let folder = if first.is_relative() {
                std::env::temp_dir().join("proceduralfx_sequence")
            } else {
                first.parent().map(PathBuf::from).unwrap_or_default()
            };
            self.status = format!("Sequence complete: {total} frames in {}", folder.display());
            self.sequence = None;
            self.render_at = None;
            return;
        }
        let index = job.next;
        job.next += 1;
        let time = job.times[index];
        let (executable, arguments, output) = (job.executable.clone(), job.arguments.clone(), frame_output(&job.output, index + 1));
        let (width, height, samples) = (job.width, job.height, job.samples);
        self.render_opts = job.opts.clone();
        self.render_at = Some(time);
        self.spawn_moonray_render(executable, arguments, output, width, height, samples);
        if self.moonray_rx.is_none() {
            // spawn_moonray_render rejected the settings and set the status.
            self.sequence = None;
            self.render_at = None;
            return;
        }
        self.status = format!("Rendering frame {}/{total} (t = {time:.3})...", index + 1);
    }
}
