#![allow(clippy::unwrap_used)]

use pigs_api::orchestration::{
    Advance, OrchestrationError, OrchestrationLimits, OrchestrationState,
};
use pigs_api::phased_phase::Phase;

fn state(max_post_iterations: u32, max_pre_replans: u32) -> OrchestrationState {
    OrchestrationState::new(OrchestrationLimits {
        max_post_iterations,
        max_pre_replans,
    })
}

#[test]
fn pre_end_completes_and_pre_without_marker_enters_executor() {
    let mut simple = state(3, 2);
    assert_eq!(
        simple.advance("simple answer\nPIGEND").unwrap(),
        Advance::Complete
    );

    let mut planned = state(3, 2);
    assert_eq!(
        planned.advance("five-part plan").unwrap(),
        Advance::Continue(Phase::Executor)
    );
    assert_eq!(planned.pre_output(), "five-part plan");
}

#[test]
fn executor_always_enters_post() {
    let mut state = state(3, 2);
    state.advance("plan").unwrap();
    assert_eq!(
        state.advance("executor result").unwrap(),
        Advance::Continue(Phase::Post)
    );
}

#[test]
fn post_end_completes_fail_replans_and_markerless_stays_in_post() {
    let mut complete = state(3, 2);
    complete.advance("plan").unwrap();
    complete.advance("executor").unwrap();
    assert_eq!(
        complete.advance("accepted\nPIGEND").unwrap(),
        Advance::Complete
    );

    let mut failed = state(3, 2);
    failed.advance("plan").unwrap();
    failed.advance("executor").unwrap();
    assert_eq!(
        failed.advance("regressed\nPIGFAIL").unwrap(),
        Advance::Continue(Phase::Pre)
    );
    assert_eq!(failed.failure_outputs(), &["regressed"]);

    let mut continuing = state(3, 2);
    continuing.advance("plan").unwrap();
    continuing.advance("executor").unwrap();
    assert_eq!(
        continuing.advance("made progress").unwrap(),
        Advance::Continue(Phase::Post)
    );
    assert_eq!(continuing.post_outputs(), &["made progress"]);
}

#[test]
fn repeated_post_outputs_and_numbered_failures_are_preserved() {
    let mut state = state(4, 2);
    state.advance("plan one").unwrap();
    state.advance("executor one").unwrap();
    state.advance("post one").unwrap();
    state.advance("post two\nPIGFAIL").unwrap();

    assert_eq!(state.post_outputs(), &["post one", "post two"]);
    assert_eq!(state.failure_outputs(), &["post two"]);
    assert_eq!(state.numbered_failures(), "第 1 次失败：\npost two");
}

#[test]
fn exhausted_budgets_are_errors_not_successful_terminal_states() {
    let mut posts = state(1, 2);
    posts.advance("plan").unwrap();
    posts.advance("executor").unwrap();
    assert_eq!(
        posts.advance("continue once").unwrap(),
        Advance::Continue(Phase::Post)
    );
    assert!(matches!(
        posts.advance("continue twice"),
        Err(OrchestrationError::PostBudgetExceeded { limit: 1 })
    ));

    let mut replans = state(3, 0);
    replans.advance("plan").unwrap();
    replans.advance("executor").unwrap();
    assert!(matches!(
        replans.advance("failed\nPIGFAIL"),
        Err(OrchestrationError::ReplanBudgetExceeded { limit: 0 })
    ));
}
