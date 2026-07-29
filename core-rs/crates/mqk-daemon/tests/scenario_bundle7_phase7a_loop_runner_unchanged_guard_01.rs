//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A structural guard.
//!
//! This patch owns only run-start/run-stop/run-halt lifecycle integration of
//! the dynamic-selection start gate. It must not alter economic tick
//! dispatch (loop_runner strategy dispatch, targets/decisions, Bundle 5/6
//! input/call counts, cap #6, outbox/broker writes). Rather than trust an
//! implementation claim, this test proves `state/loop_runner.rs` is
//! byte-for-byte identical to the patch's required starting HEAD.
//!
//! If this test ever needs to change, that is itself proof the patch scope
//! was violated — fix the patch, not the guard.

use std::process::Command;

/// Bundle 7 Phase 7A's required starting local HEAD / origin/main, per the
/// patch's REQUIRED STARTING STATE section.
const STARTING_HEAD: &str = "3f69db0e54fb8c9744eabbf26b938e57ce004d4d";
const LOOP_RUNNER_PATH: &str = "core-rs/crates/mqk-daemon/src/state/loop_runner.rs";

#[test]
fn loop_runner_is_byte_identical_to_patch_starting_head() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // CARGO_MANIFEST_DIR is .../core-rs/crates/mqk-daemon — three levels
    // below the repo root (mqk-daemon -> crates -> core-rs -> repo root).
    let repo_root = std::path::Path::new(manifest_dir)
        .ancestors()
        .nth(3)
        .expect("mqk-daemon crate must be nested three levels below the repo root")
        .to_path_buf();

    let output = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("diff")
        .arg("--exit-code")
        .arg(STARTING_HEAD)
        .arg("--")
        .arg(LOOP_RUNNER_PATH)
        .output()
        .expect("failed to invoke git (required for this structural guard)");

    assert!(
        output.status.success(),
        "state/loop_runner.rs differs from the Bundle 7 Phase 7A required \
         starting HEAD ({STARTING_HEAD}) — this patch owns lifecycle wiring \
         only and must not change economic tick dispatch.\n\
         git diff --exit-code {STARTING_HEAD} -- {LOOP_RUNNER_PATH}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
