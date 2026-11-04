// Performance checks: each preset must cook its first second within a budget, so a change
// that makes cooking much slower fails CI instead of reaching users.
//
// CI machines are faster or slower than this one, and than each other, so times are
// measured against a yardstick: a fixed piece of arithmetic that uses none of the app's
// code, timed on the same machine at the same moment. Everything runs on one thread, so
// the number of cores doesn't matter either. A budget is a multiple of the yardstick,
// about 2.5 times what the preset took when the budgets were last set, which leaves room
// for noisy shared machines but catches a real slowdown.
//
// Run: cargo test --release -- --ignored cooking_stays_within_its_budget --nocapture
// Set the budgets again (after making cooking faster, or adding a preset):
//     PFX_WRITE_BUDGETS=1 cargo test --release -- --ignored cooking_stays_within_its_budget
use crate::*;

const BUDGETS: &str = include_str!("perf_budgets.txt");
// How much room a budget leaves over what a preset took when it was set.
const ROOM: f64 = 2.5;
// The least budget, in yardsticks: below it, timings of a few milliseconds are mostly noise.
const FLOOR: f64 = 2.0;
// The frames each preset cooks: its first second.
const FRAMES: usize = 24;

// Fixed work in plain arithmetic and memory (sorting, sums): seconds, the best of five.
fn yardstick() -> f64 {
    (0..5)
        .map(|_| {
            let start = Instant::now();
            let mut seed = 0x9e37_79b9_7f4a_7c15u64;
            let mut values: Vec<f32> = (0..1_000_000)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 40) as f32 / (1u64 << 24) as f32
                })
                .collect();
            values.sort_unstable_by(f32::total_cmp);
            let total: f32 = values.iter().map(|v| v.sqrt()).sum();
            std::hint::black_box(total);
            start.elapsed().as_secs_f64()
        })
        .fold(f64::INFINITY, f64::min)
}

// Seconds to cook a preset's first second, frame by frame as playback does (each frame
// cooked afresh; simulations carry on from the frame before), from nothing: the best of three.
fn cook_time(preset: &presets::Preset) -> f64 {
    (0..3)
        .map(|_| {
            sims::clear_all();
            let graph = Graph::deserialize(preset.scene);
            let displays = graph.geometry_ids();
            let start = Instant::now();
            for frame in 0..FRAMES {
                let mesh = graph.cook_display(&displays, preset.range.0 + frame as f32 / sims::SIM_FPS, &mut HashMap::new());
                std::hint::black_box(mesh);
            }
            start.elapsed().as_secs_f64()
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
#[ignore]
fn cooking_stays_within_its_budget() {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let (unit, measured) = pool.install(|| {
        let unit = yardstick();
        let measured: Vec<(&str, f64)> = presets::PRESETS.iter().map(|p| (p.name, cook_time(p) / unit)).collect();
        (unit, measured)
    });
    eprintln!("yardstick: {:.1} ms", unit * 1000.0);
    if std::env::var_os("PFX_WRITE_BUDGETS").is_some() {
        let mut text = String::from("# Each preset's budget for cooking its first second, in yardsticks (see perf_budgets.rs).\n");
        for (name, ratio) in &measured {
            text += &format!("{name}: {:.1}\n", ((ratio * ROOM).max(FLOOR) * 10.0).ceil() / 10.0);
        }
        std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app_tests/perf_budgets.txt"), text).unwrap();
        return;
    }
    let budgets: HashMap<&str, f64> = BUDGETS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.rsplit_once(": ").map(|(name, v)| (name, v.trim().parse().unwrap())))
        .collect();
    let mut over = Vec::new();
    for (name, ratio) in &measured {
        let budget = *budgets.get(name).unwrap_or_else(|| panic!("{name} has no budget: set them again (see the top of perf_budgets.rs)"));
        eprintln!("{name:<16} {ratio:>7.1} yardsticks of {budget:>6.1} ({:>3.0}%)", ratio / budget * 100.0);
        if *ratio > budget {
            over.push(format!("{name}: {ratio:.1} yardsticks, over its budget of {budget}"));
        }
    }
    assert!(over.is_empty(), "cooking got slower: {over:?}");
}

