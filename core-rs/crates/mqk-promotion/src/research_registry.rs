use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql};

// ============================================================================
// P7C-REPAIR-03 (mission: FINAL WAVE-2 BLOCKER REPAIR, Patch B)
// ============================================================================
//
// DEFECT FIXED: `ResearchAttemptAuthority` (REPAIR-02) had public fields and
// was constructible by any caller with plain struct-literal syntax --
// `verify_promotion_oos_evidence` trusted it, but a caller could fabricate
// an economic artifact, a daily-returns CSV, a favorable judge artifact,
// and a matching `trial_id`/`economic_eval_id`/judge SHA256, then simply
// TYPE those same three strings into an `ResearchAttemptAuthority` literal
// and pass -- REPAIR-02's negative tests only ever supplied a WRONG
// authority record; none proved a matching FABRICATED authority fails.
//
// FIX: this module is the ONLY source of a [`VerifiedResearchAuthority`],
// and it is not publicly constructible -- every field is private and there
// is no `pub` constructor anywhere in this crate. The sole way to obtain one
// is [`load_research_authority`], which opens the durable Research registry
// (the SAME SQLite database `mqk_research.exp_distributed.storage.
// ResearchResultStore` writes: `research_trials`, `research_attempts`, and
// the additive `research_judge_artifacts` table this repair introduces)
// READ-ONLY, and itself establishes -- from the registry's own rows, never
// from caller-supplied claims -- that:
//   1. `research_trials` contains `trial_id` (and yields its registered
//      `experiment_id`/`hypothesis_id`);
//   2. a `research_attempts` row for `trial_id` exists with
//      `status = 'succeeded'` and `result_id` equal to the supplied
//      `economic_eval_id` (the economic artifact's OWN
//      `ids.economic_eval_id`, extracted and verified elsewhere in
//      `verify_promotion_oos_evidence` before this is called);
//   3. a `research_judge_artifacts` row exists whose
//      `judge_artifact_sha256` equals the ACTUAL supplied judge artifact's
//      canonical SHA-256 (computed by the caller the same way
//      `verify_promotion_oos_evidence` always has), and whose
//      `experiment_id`/`hypothesis_id` belongs to this trial's own
//      registered experiment (a judge run scoped to a specific
//      `hypothesis_id` must match the trial's hypothesis_id exactly; a
//      judge run scoped to the whole experiment -- `hypothesis_id` is
//      `NULL`/absent -- covers every trial in that experiment).
//
// A caller cannot satisfy any of this by typing strings: it requires a real
// row, written by the real registry write path
// (`ResearchResultStore.register_trial` / `begin_attempt`+`finalize_attempt`
// / the new `ResearchResultStore.register_judge_artifact`, called from
// `mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge`
// once the judge's own DSR/PBO values are finalized -- see that function's
// own docs). This crate has NO write path into the registry and never
// creates the schema -- it only reads tables the Python registry already
// owns; `load_research_authority` fails closed if the database file is
// unreadable, or if any table it queries does not yet exist (a stale/wrong
// registry path, never silently treated as "no facts to check").
// Fields are the confirmed facts, not currently read back by the caller
// (`verify_promotion_oos_evidence` already has `trial_id`/`economic_eval_id`
// /the actual judge hash from its own parsing) -- this is a witness type:
// its EXISTENCE (a successful `load_research_authority` call) is what
// matters, proving the three checks above all passed against real registry
// rows. Kept as a named struct, not `()`, so the registry-loading step
// stays independently testable and documented, per the mission's preferred
// design.
#[allow(dead_code)]
pub(crate) struct VerifiedResearchAuthority {
    pub(crate) trial_id: String,
    pub(crate) economic_eval_id: String,
    pub(crate) judge_artifact_sha256: String,
}

/// Open the registry read-only and establish authority for `trial_id`
/// against `economic_eval_id` (the economic artifact's own recorded
/// `ids.economic_eval_id`) and `judge_canonical_sha256` (the ACTUAL supplied
/// judge artifact's canonical SHA-256, computed by the caller). Returns one
/// human-readable reason per failing check, same fail-closed contract as
/// [`crate::research_evidence::verify_promotion_oos_evidence`].
pub(crate) fn load_research_authority(
    registry_db_path: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    judge_canonical_sha256: &str,
) -> Result<VerifiedResearchAuthority, Vec<String>> {
    let mut errs: Vec<String> = Vec::new();

    let conn = match Connection::open_with_flags(registry_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry database is unavailable at \
                 {registry_db_path:?}: {e} -- authority cannot be established without a \
                 readable durable registry"
            ));
            return Err(errs);
        }
    };

    // ---- 1. research_trials contains trial_id ----
    let trial_row = query_two_col_row(
        &conn,
        "select experiment_id, hypothesis_id from research_trials where trial_id = ?1",
        [trial_id],
        &mut errs,
        "research_trials",
    );
    let (experiment_id, hypothesis_id) = match trial_row {
        Some(v) => v,
        None => {
            if !errs.iter().any(|e| e.contains("research_trials")) {
                errs.push(format!(
                    "OOS evidence rejected: research registry has no research_trials row for \
                     trial_id {trial_id:?} -- this trial was never registered"
                ));
            }
            return Err(errs);
        }
    };

    // ---- 2. a succeeded attempt for trial_id whose result_id matches ----
    let attempt_exists = row_exists(
        &conn,
        "select 1 from research_attempts \
         where trial_id = ?1 and status = 'succeeded' and result_id = ?2 limit 1",
        [trial_id, economic_eval_id],
        &mut errs,
        "research_attempts",
    );
    if !attempt_exists && !errs.iter().any(|e| e.contains("research_attempts")) {
        errs.push(format!(
            "OOS evidence rejected: research registry has no succeeded research_attempts row \
             for trial_id {trial_id:?} with result_id {economic_eval_id:?} -- this economic \
             artifact does not correspond to any selected successful attempt of this trial"
        ));
    }

    // ---- 3. a registered judge artifact matching the actual supplied hash ----
    let judge_row = query_two_col_row(
        &conn,
        "select experiment_id, hypothesis_id from research_judge_artifacts \
         where judge_artifact_sha256 = ?1 limit 1",
        [judge_canonical_sha256],
        &mut errs,
        "research_judge_artifacts",
    );
    match judge_row {
        None => {
            if !errs.iter().any(|e| e.contains("research_judge_artifacts")) {
                errs.push(format!(
                    "OOS evidence rejected: research registry has no research_judge_artifacts \
                     row whose judge_artifact_sha256 matches the actual supplied judge \
                     artifact's canonical SHA-256 {judge_canonical_sha256:?} -- this judge \
                     artifact (including its DSR/PBO numeric output) was never durably \
                     registered"
                ));
            }
        }
        Some((judge_experiment_id, judge_hypothesis_id)) => {
            let experiment_matches = judge_experiment_id == experiment_id;
            // A judge run scoped to the whole experiment (hypothesis_id
            // NULL/empty in the registry) covers every trial in that
            // experiment; a judge run scoped to one hypothesis must match
            // the trial's own hypothesis_id exactly.
            let hypothesis_matches = match judge_hypothesis_id.as_deref() {
                None | Some("") => true,
                Some(h) => Some(h) == hypothesis_id.as_deref(),
            };
            if !experiment_matches || !hypothesis_matches {
                errs.push(format!(
                    "OOS evidence rejected: the registered judge artifact matching the supplied \
                     judge SHA-256 belongs to experiment_id={judge_experiment_id:?} \
                     hypothesis_id={judge_hypothesis_id:?}, not trial_id {trial_id:?}'s own \
                     registered experiment_id={experiment_id:?} hypothesis_id={hypothesis_id:?} \
                     -- wrong experiment/hypothesis"
                ));
            }
        }
    }

    if !errs.is_empty() {
        return Err(errs);
    }

    Ok(VerifiedResearchAuthority {
        trial_id: trial_id.to_string(),
        economic_eval_id: economic_eval_id.to_string(),
        judge_artifact_sha256: judge_canonical_sha256.to_string(),
    })
}

/// Runs a query expected to yield at most one `(text, nullable text)` row,
/// treating "table does not exist" (or any other query-preparation failure)
/// as a fail-closed registry error pushed into `errs` -- a stale/wrong
/// registry path must never be indistinguishable from "genuinely no
/// matching row".
fn query_two_col_row<P: ToSql, const N: usize>(
    conn: &Connection,
    sql: &str,
    params: [P; N],
    errs: &mut Vec<String>,
    table_name: &str,
) -> Option<(String, Option<String>)> {
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against {table_name} failed \
                 (missing/incompatible schema?): {e}"
            ));
            return None;
        }
    };
    match stmt
        .query_row(rusqlite::params_from_iter(params.iter()), |row| {
            let a: String = row.get(0)?;
            let b: Option<String> = row.get(1)?;
            Ok((a, b))
        })
        .optional()
    {
        Ok(v) => v,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against {table_name} failed: {e}"
            ));
            None
        }
    }
}

/// Same fail-closed schema-error handling as [`query_two_col_row`], but for
/// a plain existence check (`select 1 from ... limit 1`).
fn row_exists<P: ToSql, const N: usize>(
    conn: &Connection,
    sql: &str,
    params: [P; N],
    errs: &mut Vec<String>,
    table_name: &str,
) -> bool {
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against {table_name} failed \
                 (missing/incompatible schema?): {e}"
            ));
            return false;
        }
    };
    match stmt
        .query_row(rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, i64>(0)
        })
        .optional()
    {
        Ok(v) => v.is_some(),
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against {table_name} failed: {e}"
            ));
            false
        }
    }
}
