// Help → Copy diagnostics: what support needs to help with a problem short of a crash.

use crate::*;

impl ProceduralFxStudioApp {
    // The version, system, GPU and how the viewport draws, the scene's size, and the recent
    // log, with the home folder shown as ~. Copied for the user to send; never sent.
    pub(crate) fn diagnostics(&self) -> String {
        let viewport = match (&self.gpu_viewport, &self.gpu_failure) {
            (Some(_), _) => "GPU".to_owned(),
            (None, Some(error)) => format!("CPU, after a GPU error: {}", error.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" / ")),
            (None, None) => "CPU".to_owned(),
        };
        let displays = self.graph.geometry_ids().len();
        let mut text = format!(
            "ProceduralFX Studio {} diagnostics, {} UTC\nSystem: {}\nGPU: {}\nViewport: {viewport}\nScene: {} nodes, {displays} objects shown, {} saved\n\nRecent log:\n",
            env!("CARGO_PKG_VERSION"),
            applog::date_time(std::time::SystemTime::now()),
            applog::system(),
            self.gpu_info.as_deref().unwrap_or("none found"),
            self.graph.nodes.len(),
            if self.has_unsaved_changes() { "changes not" } else { "all" },
        );
        for line in applog::recent() {
            text += &line;
            text.push('\n');
        }
        recovery::redacted(&text, recovery::home_folder().as_deref())
    }
}
