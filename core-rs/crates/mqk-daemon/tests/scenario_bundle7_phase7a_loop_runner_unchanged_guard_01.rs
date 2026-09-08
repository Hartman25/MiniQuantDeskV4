//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 historical structural guard.
//!
//! This file began as a patch-boundary guard for Bundle 7 Phase 7A and was
//! narrowed again for Phase 7B. Its byte-equality claim is therefore a
//! historical claim about those frozen patch boundaries, not a permanent
//! prohibition on later accepted changes to `state/loop_runner.rs`.
//!
//! CI-R12 corrects the stale interpretation that compared the current
//! working tree forever against the Phase-7A patch-start commit. Later
//! independently accepted work legitimately added pre-B1C behavior such as
//! execution heartbeat visibility, external snapshot refresh, and pre-event
//! handling. Reverting that production behavior merely to satisfy an old
//! patch-boundary test would be incorrect.
//!
//! The durable proof is now split cleanly:
//!
//! * `FROZEN_ECONOMIC_BASELINE` remains the immutable lineage origin.
//! * `PATCH_START_HEAD` remains the Phase-7A R6 historical patch start.
//! * `PHASE_7B_START_HEAD` and `PHASE_7B_CLOSURE_HEAD` are frozen historical
//!   commits. The pre-B1C section is byte-compared between those two commits,
//!   proving Phase 7B did not silently alter the section it declared frozen.
//! * ancestry checks prove that full historical chain remains on current HEAD.
//! * the current working tree is checked structurally by the prologue test;
//!   it is deliberately not byte-compared to a months-old patch-start commit.
//!
//! The B1C economic-dispatch section remains outside this historical
//! pre-dispatch comparison. Phase 7B explicitly owned that section and its
//! behavior is covered by its dedicated closure guards and runtime tests.
//!
//! This keeps the original guard useful without allowing historical patch
//! bookkeeping to veto later accepted production changes.

use std::process::Command;

/// Permanent origin of the Bundle-7 economic-dispatch guard lineage.
const FROZEN_ECONOMIC_BASELINE: &str = "9323b7699af5e4c553522fa118a49c644a3611da";

/// Phase-7A R6 historical patch-start commit. Kept for ancestry truth only.
const PATCH_START_HEAD: &str = "a0037af74ac725366b187b0f1bf7f8944bfac1ca";

/// Frozen start of the accepted Phase-7B selected-host dispatch closure.
const PHASE_7B_START_HEAD: &str = "e0e44d2b39b38ad0f2e65c2b71306c58c962140e";

/// Frozen accepted Phase-7B closure commit.
const PHASE_7B_CLOSURE_HEAD: &str = "ccfe067ec5302c64589695377be5e3d8cdf366cd";

const LOOP_RUNNER_PATH: &str = "core-rs/crates/mqk-daemon/src/state/loop_runner.rs";

/// Current-source structural anchor for the execution-loop prologue.
const REQUIRED_TICK_LOOP_ANCHOR: &str =
    "        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);";

/// Historical pre-B1C comparison start.
const REQUIRED_DISPATCH_BODY_START_ANCHOR: &str =
    "                    match orchestrator.snapshot().await.context(\"snapshot failed\") {";

/// Historical Phase-7B ownership boundary. Everything from this B1C marker
/// onward was explicitly Phase-7B-owned and is excluded from the historical
/// pre-dispatch byte comparison.
const PHASE_7B_OWNED_SECTION_ANCHOR: &str =
    "                    // B1C: Dispatch pending strategy bar input and submit Live-intent";
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

fn file_at_head(repo_root: &std::path::Path, head: &str, label: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("show")
        .arg(format!("{head}:{LOOP_RUNNER_PATH}"))
        .output()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        output.status.success(),
        "failed to read {LOOP_RUNNER_PATH} at {label} ({head}): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap_or_else(|_| panic!("{LOOP_RUNNER_PATH} at {label} must be valid UTF-8"))
}
/// Extracts the post-tick-success snapshot/outbox-reconciliation section:
/// from `REQUIRED_DISPATCH_BODY_START_ANCHOR` up to (not including)
/// `PHASE_7B_OWNED_SECTION_ANCHOR`. PHASE-7B-SELECTED-HOST-ECONOMIC-
/// DISPATCH-CLOSURE narrowing: the B1C dispatch section itself (from the
/// `PHASE_7B_OWNED_SECTION_ANCHOR` line to the loop's close) is Phase-7B's
/// authorized, wider scope and is deliberately excluded — see module doc.
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
    let end_idx = content
        .find(PHASE_7B_OWNED_SECTION_ANCHOR)
        .unwrap_or_else(|| {
            panic!(
                "{label}: PHASE_7B_OWNED_SECTION_ANCHOR not found — \
                 PHASE_7B_OWNED_SECTION_ANCHOR={PHASE_7B_OWNED_SECTION_ANCHOR:?} \
                 must exist verbatim in {LOOP_RUNNER_PATH}"
            )
        });
    assert!(
        end_idx > start_idx,
        "{label}: PHASE_7B_OWNED_SECTION_ANCHOR must appear strictly after \
         REQUIRED_DISPATCH_BODY_START_ANCHOR"
    );
    content[start_idx..end_idx].to_string()
}

#[test]
fn phase7b_pre_dispatch_section_is_historically_byte_identical_across_closure() {
    let repo_root = repo_root();
    let phase7b_start_content =
        file_at_head(&repo_root, PHASE_7B_START_HEAD, "PHASE_7B_START_HEAD");
    let phase7b_closure_content =
        file_at_head(&repo_root, PHASE_7B_CLOSURE_HEAD, "PHASE_7B_CLOSURE_HEAD");

    let phase7b_start_body = dispatch_body_from(&phase7b_start_content, "PHASE_7B_START_HEAD");
    let phase7b_closure_body =
        dispatch_body_from(&phase7b_closure_content, "PHASE_7B_CLOSURE_HEAD");

    assert_eq!(
        phase7b_closure_body, phase7b_start_body,
        "Bundle 7 Phase 7B changed the historical post-tick-success \
         snapshot/outbox-reconciliation section before its B1C-owned \
         dispatch boundary; start={PHASE_7B_START_HEAD}, \
         closure={PHASE_7B_CLOSURE_HEAD}"
    );
}

fn assert_ancestor(repo_root: &std::path::Path, ancestor: &str, descendant: &str, label: &str) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(ancestor)
        .arg(descendant)
        .status()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        status.success(),
        "{label}: expected {ancestor} to be an ancestor of {descendant}"
    );
}

fn current_head(repo_root: &std::path::Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .expect("failed to invoke git (required for this structural guard)");
    assert!(
        output.status.success(),
        "failed to resolve current HEAD: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git rev-parse HEAD output must be UTF-8")
        .trim()
        .to_string()
}

#[test]
fn historical_guard_commits_are_on_current_lineage() {
    let repo_root = repo_root();
    let head = current_head(&repo_root);

    assert_ancestor(
        &repo_root,
        FROZEN_ECONOMIC_BASELINE,
        PATCH_START_HEAD,
        "frozen baseline -> Phase 7A patch start",
    );
    assert_ancestor(
        &repo_root,
        PATCH_START_HEAD,
        PHASE_7B_START_HEAD,
        "Phase 7A patch start -> Phase 7B start",
    );
    assert_ancestor(
        &repo_root,
        PHASE_7B_START_HEAD,
        PHASE_7B_CLOSURE_HEAD,
        "Phase 7B start -> Phase 7B closure",
    );
    assert_ancestor(
        &repo_root,
        PHASE_7B_CLOSURE_HEAD,
        &head,
        "Phase 7B closure -> current HEAD",
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
        // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 1/3: the
        // frozen per-symbol assignment parameter was replaced by the one
        // frozen dispatch authority (which carries the exact same
        // assignments for Legacy, plus the selected-host authority for
        // DynamicPaperEnforced) — an authorized, documented Phase 7B
        // signature change, not a silent regression.
        prologue.contains("dispatch_authority: RuntimeStrategyDispatchAuthority"),
        "the frozen dispatch-authority parameter must still be declared \
         before the unchanged pre-dispatch-body prologue"
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
