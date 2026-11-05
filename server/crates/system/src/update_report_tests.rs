//! The update record: helper logs read back, the failing step and its error, and the version check of the
//! server that came back.
use super::*;

/// A helper's `docker logs`, as the tracing-free helper prints them.
fn helper_log(lines: &[&str]) -> String {
    lines.iter().map(|l| format!("{LINE_PREFIX}{l}\n")).collect()
}

fn started() -> Report {
    let mut r = Report::start("ghcr.io/krifmaz/encore:0.42.18", "0.42.18", "0.42.15", "admin", 1000.0);
    r.line("pull", "ghcr.io/krifmaz/encore:0.42.18 is on this host");
    r
}

/// The bug this record was made for: 0.42.18 (built from older code) refused the database, the helper rolled
/// back, and the page only said "did not go through". Now the step, the error and the new server's own words
/// are in the record.
#[test]
fn a_rolled_back_swap_names_its_step_and_error() {
    let log = helper_log(&[
        "inspect: updating container 784f60a52647 to ghcr.io/krifmaz/encore:0.42.18",
        "stop: stopping encore (up to 30 s)",
        "start: starting encore (9b1e8f2a7c6d)",
        "verify: new container: This database was written by a newer Encore (schema 42, this server knows 41).",
        "verify: the new container stopped (status exited, exit code 3): the new Encore refused the database",
        "rollback: encore runs again on the old image",
        "failed: verify: the new container stopped (status exited, exit code 3): the new Encore refused the database",
    ]);
    let mut r = started();
    merge_helper(&mut r, &log, 1, 2000.0);
    assert_eq!(r.state, State::Failed);
    assert_eq!(r.failed_step.as_deref(), Some("verify"));
    assert_eq!(r.error.as_deref(), Some("the new container stopped (status exited, exit code 3): the new Encore refused the database"));
    assert_eq!(r.finished_at, Some(2000.0));
    assert!(r.lines.iter().any(|l| l.contains("schema 42, this server knows 41")), "{:?}", r.lines);
    // the version check does not overwrite the helper's own reason
    verify(&mut r, "0.42.15", true, 2001.0);
    assert_eq!(r.failed_step.as_deref(), Some("verify"));
    let text = r.text();
    assert!(text.starts_with("Encore update 0.42.15 → 0.42.18 (ghcr.io/krifmaz/encore:0.42.18), started by admin\nFailed at step: verify"), "{text}");
    assert!(text.contains("Failed at step: verify\nError: the new container stopped"), "{text}");
    assert!(text.contains("  helper: rollback: encore runs again on the old image\n"), "{text}");
}

/// A helper before the step names wrote its error without one: filed under `swap`, the words kept.
#[test]
fn an_older_helpers_error_is_kept_as_it_is() {
    let (lines, failure) = parse_helper_log("garbage\nencore self-update: stopping encore\nencore self-update: failed: the new container stopped (status exited, exit code 3)\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(failure, Some(("swap".into(), "the new container stopped (status exited, exit code 3)".into())));
    let (_, failure) = parse_helper_log("encore self-update: failed: create: Docker answered 409: Conflict");
    assert_eq!(failure, Some(("create".into(), "Docker answered 409: Conflict".into())));
}

/// A helper that ended badly without a word, a server that came back on the old version, and one that came
/// back on the new one.
#[test]
fn the_server_that_comes_back_checks_its_version() {
    let mut r = started();
    merge_helper(&mut r, "", 137, 2000.0);
    assert_eq!((r.state, r.failed_step.as_deref()), (State::Failed, Some("helper")));
    assert!(r.error.as_deref().unwrap().contains("exit code 137"));

    // the helper's log is gone (an older server made it --rm): the old version back means it failed
    let mut r = started();
    verify(&mut r, "0.42.15", false, 1100.0);
    assert_eq!(r.state, State::Running, "too early to tell: the helper may still be on its way");
    verify(&mut r, "0.42.15", false, 1000.0 + LOST_AFTER_SECS + 1.0);
    assert_eq!((r.state, r.failed_step.as_deref()), (State::Failed, Some("verify")));
    assert!(r.error.as_deref().unwrap().contains("came back on Encore 0.42.15, not 0.42.18"));

    // a clean swap: the helper said nothing went wrong and the new version answers
    let mut r = started();
    merge_helper(&mut r, &helper_log(&["cleanup: done"]), 0, 2000.0);
    assert_eq!(r.state, State::Running);
    verify(&mut r, "0.42.18", true, 2001.0);
    assert_eq!((r.state, r.finished_at), (State::Ok, Some(2001.0)));
    assert_eq!(r.lines.last().map(String::as_str), Some("verify: the server answers as Encore 0.42.18"));
}

/// A pull that failed on the server ends the record there; the JSON carries the text Copy takes.
#[test]
fn a_failed_pull_is_the_failing_step() {
    let mut r = started();
    r.fail("pull", "Could not pull ghcr.io/krifmaz/encore:0.42.18: manifest unknown", 1001.0);
    let v = json(Some(&r));
    assert_eq!(v["state"], "failed");
    assert_eq!(v["failed_step"], "pull");
    assert!(v["text"].as_str().unwrap().contains("Error: Could not pull"));
    assert_eq!(json(None), Value::Null);
}
