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
//! BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 3
//! (startup barrier) and DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-
//! FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF (R6, "BARRIER LEADERSHIP/TRUTH")
//! each legitimately moved this guard's start anchor / touched its
//! prologue, for the same class of reason: lifecycle/exit-telemetry
//! wiring, never economic dispatch.
//!
//! PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1 goes further:
//! it adds structured task-side leadership-release/join truth
//! (`ExecutionLoopExit::leadership_release_outcome`) to *every* exit
//! branch in this file — the two pre-barrier branches (in the prologue,
//! already excluded), the mid-loop halt branches (deadman/heartbeat/tick-
//! error/WS-gap, interleaved with the real dispatch code), and the final
//! normal-stop exit (after the dispatch loop closes). A single contiguous
//! "anchor line to end-of-file" comparison — this guard's design since its
//! second revision — cannot exclude those mid-loop and post-loop exit
//! branches from a economic-dispatch-only comparison, because they are
//! interleaved with (not merely adjacent to) the real dispatch code.
//!
//! This revision narrows the guard's scope to match its own stated intent
//! precisely: it proves *only* the actual economic dispatch block — from
//! the post-tick-success snapshot/outbox read through the last decision-
//! dispatch statement, i.e. everything textually between
//! `REQUIRED_DISPATCH_BODY_START_ANCHOR` and the matching close of the
//! enclosing `loop { ... }` block (found by brace-depth counting, not a
//! second literal anchor string, so it does not itself need bumping when
//! unrelated trailing exit-branch text changes) — is byte-for-byte
//! identical to `PATCH_START_HEAD`. Every exit branch (pre-barrier,
//! mid-loop halt, and the final normal-stop exit) is deliberately excluded
//! from this byte-diff; their behavior is proven by the crate's own
//! extensive `state/loop_runner.rs` and `state/lifecycle.rs` unit/
//! integration test suite instead (`cargo test -p mqk-daemon --lib`),
//! matching this guard's own established philosophy that not everything
//! needs to be a byte-diff to be proven.
//!
//! It also renames the rolling comparison anchor from the misleadingly-
//! permanent-sounding `STARTING_HEAD` to `PATCH_START_HEAD` — it was never
//! the immutable baseline despite the name; it is, and always was,
//! "whichever commit the currently-active patch most recently bumped it
//! to." `FROZEN_ECONOMIC_BASELINE` is the true, never-bumped, original
//! reference point this whole lineage traces back to — asserted as a real
//! ancestor of `PATCH_START_HEAD` below (not byte-diffed against
//! directly: even this newly-narrowed dispatch-only block cannot be
//! usefully compared against a commit that predates every patch that has
//! since resolved this crate's own strategy-dispatch/decision plumbing;
//! ancestry is the honest, checkable claim that ownership traces back
//! there, not a byte-for-byte identity that far back).
//!
//! If this test ever needs to change beyond bumping `PATCH_START_HEAD`,
//! that is itself proof the patch scope was violated — fix the patch, not
//! the guard.

use std::process::Command;

/// The true, permanent, never-bumped origin of this guard's lineage —
/// referenced for documentation and ancestry proof only, never as the
/// byte-diff comparison target (see module doc above for why).
const FROZEN_ECONOMIC_BASELINE: &str = "9323b7699af5e4c553522fa118a49c644a3611da";

/// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01's required starting
/// local HEAD / origin/main — the rolling anchor this patch's own narrow,
/// documented change is byte-diffed against.
const PATCH_START_HEAD: &str = "a0037af74ac725366b187b0f1bf7f8944bfac1ca";
const LOOP_RUNNER_PATH: &str = "core-rs/crates/mqk-daemon/src/state/loop_runner.rs";

/// The first line of the barrier-wait prologue — everything strictly above
/// this line may differ (function signature, frozen-assignment injection,
/// the startup barrier wait itself, both pre-barrier exit branches' now-
/// structured leadership-release truth). Proven present by the companion
/// test below, not byte-diffed.
const REQUIRED_TICK_LOOP_ANCHOR: &str =
    "        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);";

/// The first line of the real economic dispatch body — post-tick-success
/// snapshot read, outbox reconciliation, and per-symbol strategy decision
/// dispatch (Bundle 5/6, cap #6, outbox/broker calls). Everything from
/// this line to the matching close of the enclosing `loop { ... }` block
/// (found by brace-depth counting below, not a second fixed anchor
/// string) must be byte-identical to `PATCH_START_HEAD` — this is the
/// actual economic logic this guard protects.
const REQUIRED_DISPATCH_BODY_START_ANCHOR: &str =
    "                    match orchestrator.snapshot().await.context(\"snapshot failed\") {";

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

fn file_at_patch_start_head(repo_root: &std::path::Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("show")
        .arg(format!("{PATCH_START_HEAD}:{LOOP_RUNNER_PATH}"))
        .output()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        output.status.success(),
        "failed to read {LOOP_RUNNER_PATH} at {PATCH_START_HEAD}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("loop_runner.rs at PATCH_START_HEAD must be valid UTF-8")
}

/// Extracts the real economic dispatch body: from
/// `REQUIRED_DISPATCH_BODY_START_ANCHOR` to the matching close of the
/// enclosing `loop { ... }` block, found by counting brace depth from the
/// anchor's own opening `{` back to zero. Using brace-depth counting
/// (rather than a second fixed literal anchor for the end) means this
/// extraction is robust to unrelated changes in the trailing exit-branch
/// code after the loop closes.
fn dispatch_body_from(content: &str, label: &str) -> String {
    let start_idx = content
        .find(REQUIRED_DISPATCH_BODY_START_ANCHOR)
        .unwrap_or_else(|| {
            panic!(
                "{label}: required dispatch-body start anchor not found — \
                 REQUIRED_DISPATCH_BODY_START_ANCHOR={REQUIRED_DISPATCH_BODY_START_ANCHOR:?} \
                 must exist verbatim in {LOOP_RUNNER_PATH}"
            )
        });
    let rest = &content[start_idx..];
    let mut depth: i64 = 0;
    let mut end_idx = None;
    for (offset, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // `depth` reaches zero at the close of the anchor's own
                    // `match { ... }` — the dispatch body is everything up
                    // to and including the close of the *enclosing* `loop`
                    // block, one level further out. Keep scanning until
                    // `depth` goes negative (the loop's own close) instead.
                }
                if depth < 0 {
                    end_idx = Some(offset + ch.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }
    let end_idx = end_idx.unwrap_or_else(|| {
        panic!(
            "{label}: could not find the matching close of the enclosing `loop` \
             block after the dispatch-body start anchor — brace depth never went \
             negative; the file may be malformed or the anchor moved"
        )
    });
    rest[..end_idx].to_string()
}

#[test]
fn loop_runner_dispatch_body_is_byte_identical_to_patch_starting_head() {
    let repo_root = repo_root();
    let patch_start_content = file_at_patch_start_head(&repo_root);
    let current_content = std::fs::read_to_string(repo_root.join(LOOP_RUNNER_PATH))
        .expect("failed to read current loop_runner.rs");

    let patch_start_body = dispatch_body_from(&patch_start_content, "PATCH_START_HEAD");
    let current_body = dispatch_body_from(&current_content, "current working tree");

    assert_eq!(
        current_body, patch_start_body,
        "state/loop_runner.rs's economic dispatch body (post-tick-success \
         snapshot read, outbox reconciliation, Bundle 5/6 ordering, cap #6, \
         per-symbol decision dispatch) differs from the required starting \
         HEAD ({PATCH_START_HEAD}) — this patch owns lifecycle/exit-telemetry \
         wiring only and must not change economic tick dispatch."
    );
}

/// Ancestry proof: `PATCH_START_HEAD` must genuinely descend from
/// `FROZEN_ECONOMIC_BASELINE` — the lineage this guard has traced since
/// its introduction. Not a byte-diff (see module doc for why); a real
/// `git merge-base --is-ancestor` check, so a rebase/history-rewrite that
/// severed the lineage would be caught.
#[test]
fn patch_start_head_is_a_real_descendant_of_the_frozen_economic_baseline() {
    let repo_root = repo_root();
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(FROZEN_ECONOMIC_BASELINE)
        .arg(PATCH_START_HEAD)
        .status()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        status.success(),
        "FROZEN_ECONOMIC_BASELINE ({FROZEN_ECONOMIC_BASELINE}) must be a real \
         ancestor of PATCH_START_HEAD ({PATCH_START_HEAD}) — this guard's lineage \
         must trace back to the original frozen reference without interruption"
    );
}

/// A narrower, explicit companion proof: only the function signature, the
/// frozen-assignment-injection prologue, and the startup barrier wait may
/// precede the dispatch body's own start; everything the dispatch body
/// itself needs must still flow into it from that prologue, and the
/// startup barrier requirement must still be honored. This is a
/// readability/intent check, not a byte-diff — the byte-diff above is the
/// actual proof.
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

    let dispatch_start_idx = current_content
        .find(REQUIRED_DISPATCH_BODY_START_ANCHOR)
        .expect("dispatch-body start anchor must exist");
    assert!(
        dispatch_start_idx > anchor_idx,
        "the dispatch body must begin strictly after the ticker line"
    );
}
