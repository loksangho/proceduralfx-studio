use crate::*;

#[test]
fn set_up_failures_say_what_happened() {
    assert!(setup_failure(Some(0xC000_0135u32 as i32)).contains("wslcsdk.dll"));
    assert!(setup_failure(Some(3)).contains("aren't installed"));
    assert!(setup_failure(Some(-5)).contains("crashed"));
}

#[test]
fn arrows_move_through_search_results() {
    assert_eq!(search_step(0, 5, true, false), 1);
    assert_eq!(search_step(4, 5, true, false), 4, "stops at the last result");
    assert_eq!(search_step(0, 5, false, true), 0, "stops at the first");
    assert_eq!(search_step(3, 5, false, true), 2);
    // Fewer results than before: the highlight comes back into range.
    assert_eq!(search_step(9, 3, false, false), 2);
    assert_eq!(search_step(0, 0, true, false), 0);
}
