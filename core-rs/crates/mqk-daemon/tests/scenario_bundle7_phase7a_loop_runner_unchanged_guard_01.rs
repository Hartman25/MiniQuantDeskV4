//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A structural guard.
//!
//! This patch (and the ATOMICITY-SINGLE-SNAPSHOT-REPAIR patch that follows
//! it) own only run-start/run-stop/run-halt lifecycle integration of the
//! dynamic-selection start gate. Neither must alter economic tick dispatch
//! (loop_runner strategy dispatch, targets/decisions, Bundle 5/6 input/call
//! counts, cap #6, outbox/broker writes).
//!
//! The original version of this guard proved the *entire file*
//! byte-for-byte identical to the patch's starting HEAD. The
//! ATOMICITY-SINGLE-SNAPSHOT-REPAIR patch legitimately changes
//! `spawn_execution_loop`'s signature and prologue — it now receives the
//! per-symbol dispatch assignment list as a parameter (frozen once by the
//! caller's `StartAttemptAuthoritySnapshot`) instead of re-resolving it
//! from env/watchlist state internally — so a whole-file byte-identical
//! guard would fail on an in-scope, narrow, additive change.
//!
//! This narrower guard instead proves that the *tick loop body* — the
//! entire `tokio::spawn(async move { ... })` block below the frozen-
//! assignment/startup-barrier prologue — is byte-for-byte identical to the
//! same required starting HEAD. That block is the actual economic dispatch
//! path: strategy dispatch, targets/decisions, Bundle 5/6 ordering, cap #6,
//! outbox writes, and broker calls. Only frozen-assignment injection
//! (the function signature and the one-time prologue that used to resolve
//! it internally) may differ; the anchor line below is the first line of
//! the unchanged region and must never itself be edited by any future
//! patch without updating `REQUIRED_TICK_LOOP_ANCHOR` here deliberately.
//!
//! If this test ever needs to change beyond bumping `STARTING_HEAD`, that
//! is itself proof the patch scope was violated — fix the patch, not the
//! guard.
//!
//! BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 3
//! (startup barrier) legitimately moves this guard's anchor a second time,
//! for the same class of reason the ATOMICITY-SINGLE-SNAPSHOT-REPAIR patch
//! moved it the first time: the task must wait on a startup barrier
//! (raced against its own stop signal) *before* the ticker is ever
//! created — that wait, by construction, sits between the previous anchor
//! (`tokio::spawn(async move {`) and the ticker line. The anchor now starts
//! at the ticker line itself; the barrier-wait block above it is proven
//! present by a companion assertion below, and its own behavior (zero
//! economic work before barrier release) is proven by dedicated unit tests
//! in `state/loop_runner.rs`, not by this structural byte-diff.

use std::process::Command;

/// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE's required starting
/// local HEAD / origin/main.
const STARTING_HEAD: &str = "294ce902951da96ad30a915d0c2ed924eb5147db";
const LOOP_RUNNER_PATH: &str = "core-rs/crates/mqk-daemon/src/state/loop_runner.rs";

/// The first line of the unchanged tick-loop-body region. Everything from
/// this exact line (inclusive) to end-of-file must be byte-identical to
/// `STARTING_HEAD` — this is the real economic dispatch body (Bundle 5/6
/// ordering, cap #6, outbox, broker calls), never touched by lifecycle-only
/// wiring patches. The startup-barrier wait (requirement 3) sits entirely
/// above this anchor, in the prologue.
const REQUIRED_TICK_LOOP_ANCHOR: &str =
    "        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);";

fn repo_root() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // CARGO_MANIFEST_DIR is .../core-rs/crates/mqk-daemon — three levels
    // below the repo root (mqk-daemon -> crates -> core-rs -> repo root).
    std::path::Path::new(manifest_dir)
        .ancestors()
        .nth(3)
        .expect("mqk-daemon crate must be nested three levels below the repo root")
        .to_path_buf()
}

fn file_at_starting_head(repo_root: &std::path::Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("show")
        .arg(format!("{STARTING_HEAD}:{LOOP_RUNNER_PATH}"))
        .output()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        output.status.success(),
        "failed to read {LOOP_RUNNER_PATH} at {STARTING_HEAD}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("loop_runner.rs at STARTING_HEAD must be valid UTF-8")
}

fn tick_loop_body_from(content: &str, label: &str) -> String {
    let idx = content.find(REQUIRED_TICK_LOOP_ANCHOR).unwrap_or_else(|| {
        panic!(
            "{label}: required anchor line not found — \
             REQUIRED_TICK_LOOP_ANCHOR={REQUIRED_TICK_LOOP_ANCHOR:?} \
             must exist verbatim in {LOOP_RUNNER_PATH}"
        )
    });
    content[idx..].to_string()
}

#[test]
fn loop_runner_tick_body_is_byte_identical_to_patch_starting_head() {
    let repo_root = repo_root();
    let starting_head_content = file_at_starting_head(&repo_root);
    let current_content = std::fs::read_to_string(repo_root.join(LOOP_RUNNER_PATH))
        .expect("failed to read current loop_runner.rs");

    let starting_head_body = tick_loop_body_from(&starting_head_content, "STARTING_HEAD");
    let current_body = tick_loop_body_from(&current_content, "current working tree");

    assert_eq!(
        current_body, starting_head_body,
        "state/loop_runner.rs's tick loop body (economic dispatch: strategy \
         dispatch, targets/decisions, Bundle 5/6 ordering, cap #6, outbox, \
         broker calls) differs from the required starting HEAD ({STARTING_HEAD}) \
         from the anchor line onward — this patch owns lifecycle/frozen-\
         assignment-injection wiring only and must not change economic tick \
         dispatch."
    );
}

/// A narrower, explicit companion proof: only the function signature and
/// the one-time prologue above the anchor may differ, and that diff must be
/// the specific frozen-assignment-injection change (function parameter
/// list) — never a change to how the assignment list itself flows into the
/// unchanged tick loop body. This is a readability/intent check, not a
/// byte-diff — the byte-diff above is the actual proof.
#[test]
fn loop_runner_prologue_still_defines_multi_symbol_assignments_before_the_anchor() {
    let repo_root = repo_root();
    let current_content = std::fs::read_to_string(repo_root.join(LOOP_RUNNER_PATH))
        .expect("failed to read current loop_runner.rs");
    let anchor_idx = current_content
        .find(REQUIRED_TICK_LOOP_ANCHOR)
        .expect("anchor line must exist");
    let prologue = &current_content[..anchor_idx];

    assert!(
        prologue.contains("multi_symbol_assignments: Vec<super::SymbolStrategyAssignment>"),
        "the frozen assignment parameter must still be declared before the \
         unchanged tick loop body"
    );
    assert!(
        prologue.contains("fn spawn_execution_loop("),
        "spawn_execution_loop's signature must still precede the anchor"
    );
    assert!(
        prologue.contains("start_barrier: tokio::sync::oneshot::Receiver<()>"),
        "requirement 3: the startup barrier parameter must still be declared \
         before the unchanged tick loop body"
    );
    assert!(
        prologue.contains("tokio::select!") && prologue.contains("barrier_result = start_barrier"),
        "requirement 3: the task must still wait on the startup barrier \
         (raced against its own stop signal) strictly before the ticker \
         line the anchor now starts at"
    );
}
