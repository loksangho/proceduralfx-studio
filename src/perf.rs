// The performance monitor: how long each node takes to cook.
//
// Every real cook (not a cache hit) is timed. A node's own ("self") time excludes
// the time spent cooking its inputs, so the slow node is the one that's slow, not
// everything downstream of it. Statistics build up while recording is on, until
// they're cleared.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub type NodeId = usize;

#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub cooks: u64,
    // Seconds of the node's own work: total over all cooks, the latest cook, and the slowest.
    pub total: f64,
    pub last: f64,
    pub max: f64,
    // The latest cook including its inputs.
    pub last_with_inputs: f64,
}

impl Stat {
    pub fn average(&self) -> f64 {
        if self.cooks == 0 { 0.0 } else { self.total / self.cooks as f64 }
    }
}

static RECORDING: AtomicBool = AtomicBool::new(true);
static STATS: Mutex<Option<HashMap<NodeId, Stat>>> = Mutex::new(None);

thread_local! {
    // The cooks in progress on this thread: (node, start, seconds spent in its inputs).
    static STACK: RefCell<Vec<(NodeId, Instant, f64)>> = const { RefCell::new(Vec::new()) };
}

pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

pub fn clear() {
    if let Ok(mut stats) = STATS.lock() {
        *stats = None;
    }
}

// Ends the node's timing when dropped (also when the cook returns early).
pub struct Timer(bool);

pub fn start(id: NodeId) -> Timer {
    if !recording() {
        return Timer(false);
    }
    STACK.with(|s| s.borrow_mut().push((id, Instant::now(), 0.0)));
    Timer(true)
}

impl Drop for Timer {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let Some((id, started, inputs)) = STACK.with(|s| s.borrow_mut().pop()) else { return };
        let elapsed = started.elapsed().as_secs_f64();
        let own = (elapsed - inputs).max(0.0);
        // The node that asked for this one waited all of `elapsed`.
        STACK.with(|s| {
            if let Some(parent) = s.borrow_mut().last_mut() {
                parent.2 += elapsed;
            }
        });
        if let Ok(mut stats) = STATS.lock() {
            let stat = stats.get_or_insert_with(HashMap::new).entry(id).or_default();
            stat.cooks += 1;
            stat.total += own;
            stat.last = own;
            stat.max = stat.max.max(own);
            stat.last_with_inputs = elapsed;
        }
    }
}

// Every node's statistics, slowest (by total own time) first.
pub fn snapshot() -> Vec<(NodeId, Stat)> {
    let mut all: Vec<(NodeId, Stat)> = STATS.lock().ok().and_then(|s| s.as_ref().map(|m| m.iter().map(|(k, v)| (*k, *v)).collect())).unwrap_or_default();
    all.sort_by(|a, b| b.1.total.total_cmp(&a.1.total).then(a.0.cmp(&b.0)));
    all
}

pub fn get(id: NodeId) -> Option<Stat> {
    STATS.lock().ok()?.as_ref()?.get(&id).copied()
}

pub fn ms(seconds: f64) -> String {
    let ms = seconds * 1000.0;
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.2} ms")
    }
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn busy(seconds: f64) {
        let t = Instant::now();
        while t.elapsed().as_secs_f64() < seconds {}
    }

    #[test]
    fn own_time_excludes_inputs() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        {
            let _outer = start(1001);
            busy(0.004);
            {
                let _inner = start(1002);
                busy(0.02);
            }
        }
        let outer = get(1001).unwrap();
        let inner = get(1002).unwrap();
        assert_eq!((outer.cooks, inner.cooks), (1, 1));
        assert!(inner.last >= 0.02 && outer.last < 0.015, "{outer:?} {inner:?}");
        assert!(outer.last_with_inputs >= 0.024);
        // Slowest first (other tests cook at the same time, so compare just these two).
        let order: Vec<NodeId> = snapshot().into_iter().map(|(id, _)| id).filter(|id| *id == 1001 || *id == 1002).collect();
        assert_eq!(order, [1002, 1001]);
        // Paused: nothing is recorded.
        set_recording(false);
        drop(start(1003));
        set_recording(true);
        assert!(get(1003).is_none());
        clear();
        assert!(snapshot().is_empty());
        assert_eq!(ms(0.0123), "12 ms");
        assert_eq!(ms(1.5), "1.50 s");
    }
}
