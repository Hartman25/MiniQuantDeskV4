//! PAPER-SOAK-OUTBOX-ENQUEUE-RUN-STATE-FENCE-01 -- production writer guard.
//!
//! `mqk_db::outbox_enqueue` proves only that `run_id` exists as an FK
//! target, never that the durable run is `RUNNING` -- see
//! `mqk-db/tests/scenario_outbox_enqueue_run_state_fence_01.rs` for the race
//! this closes. Every production economic-intent writer in this crate
//! (signal admission, manual order submit/cancel, operator flatten,
//! pre-event flatten) has been migrated to the run-state-fenced
//! `mqk_db::outbox_enqueue_for_running_run`. `outbox_enqueue` itself
//! remains available, unfenced, only for test-fixture seeding against runs
//! that are deliberately not `RUNNING` (e.g. constructing a `SENT` row for
//! `recover_oms_and_portfolio` fixtures) -- those call sites are gated
//! `#[cfg(test)]`, either at the module level (`hermetic_positive_proofs`,
//! declared `#[cfg(test)] mod hermetic_positive_proofs;` in `state.rs`) or
//! at the enclosing `mod` level within the file
//! (`snapshot.rs::tests`/`paper_portfolio_accounting.rs::fc3c_canonical_replay_parity_tests`).
//!
//! This test is a fail-closed guard against a *new* production call site
//! silently reintroducing the unfenced primitive: it scans every `.rs` file
//! under this crate's `src/` for the literal call pattern `outbox_enqueue(`
//! (a plain substring search never matches the longer
//! `outbox_enqueue_for_running_run(` name, since the character immediately
//! after `outbox_enqueue` in that name is `_`, not `(`) and requires the
//! matching file to be on an explicit, justified allowlist. Adding a call
//! site outside that allowlist without updating it -- and without the same
//! justification this doc comment gives -- fails this test.
//!
//! Deliberately file-scoped, not line-number-scoped (CLAUDE.md diff-review
//! guidance: line-anchored assertions rot silently as surrounding code
//! shifts); a source-parsing "is this line inside `#[cfg(test)]`" checker
//! would be more precise but is unjustified complexity for a three-file
//! allowlist that changes only when a new fixture module is added.

use std::path::{Path, PathBuf};

/// Every `src/` file under this crate that is allowed to call the raw,
/// unfenced `outbox_enqueue(` -- each verified at patch time to be
/// test-fixture-only (`#[cfg(test)]`, module- or file-gated).
const ALLOWED_UNFENCED_CALLERS: &[&str] = &[
    "state/snapshot.rs",
    "state/paper_portfolio_accounting.rs",
    "state/hermetic_positive_proofs.rs",
];

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("guard: failed to read_dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("guard: failed to read dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn t10_every_unfenced_outbox_enqueue_caller_in_src_is_allowlisted_test_fixture() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");

    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);
    assert!(
        !files.is_empty(),
        "guard: found zero .rs files under {} -- the walk itself is broken",
        src_dir.display()
    );

    let mut unexpected_callers: Vec<String> = Vec::new();
    let mut confirmed_allowed: Vec<&str> = Vec::new();

    for file in &files {
        let content = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("guard: failed to read {}: {e}", file.display()));

        // A plain substring match: "outbox_enqueue(" never matches inside
        // "outbox_enqueue_for_running_run(" because the byte immediately
        // following "outbox_enqueue" there is '_', not '('.
        if !content.contains("outbox_enqueue(") {
            continue;
        }

        let rel = file
            .strip_prefix(&src_dir)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");

        if ALLOWED_UNFENCED_CALLERS.contains(&rel.as_str()) {
            confirmed_allowed.push(
                ALLOWED_UNFENCED_CALLERS
                    .iter()
                    .find(|a| **a == rel)
                    .unwrap(),
            );
        } else {
            unexpected_callers.push(rel);
        }
    }

    assert!(
        unexpected_callers.is_empty(),
        "guard: found production call site(s) using the unfenced mqk_db::outbox_enqueue() \
         outside the justified test-fixture allowlist: {unexpected_callers:?}. Every production \
         economic-intent writer must use mqk_db::outbox_enqueue_for_running_run() instead -- see \
         PAPER-SOAK-OUTBOX-ENQUEUE-RUN-STATE-FENCE-01. If this is a genuine new test-fixture \
         call site, gate it #[cfg(test)] and add it to ALLOWED_UNFENCED_CALLERS with the same \
         justification given in this file's module doc comment."
    );

    assert_eq!(
        confirmed_allowed.len(),
        ALLOWED_UNFENCED_CALLERS.len(),
        "guard: every entry in ALLOWED_UNFENCED_CALLERS must still be found calling \
         outbox_enqueue( -- if a listed file no longer calls it (e.g. the fixture was removed \
         or itself migrated), narrow the allowlist rather than leaving a stale entry"
    );
}
