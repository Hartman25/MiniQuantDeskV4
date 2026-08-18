use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql};
use serde_json::Value;

use crate::research_evidence::sha256_hex;

// ============================================================================
// P7C-REPAIR-03 (mission: FINAL WAVE-2 BLOCKER REPAIR, Patch B)
// P7C-REPAIR-04 (mission: FINAL WAVE-2 + MASTER-LEDGER CONSOLIDATION, Patch A)
// ============================================================================
//
// DEFECT FIXED (REPAIR-03): `ResearchAttemptAuthority` (REPAIR-02) had public
// fields and was constructible by any caller with plain struct-literal
// syntax -- `verify_promotion_oos_evidence` trusted it, but a caller could
// fabricate an economic artifact, a daily-returns CSV, a favorable judge
// artifact, and a matching `trial_id`/`economic_eval_id`/judge SHA256, then
// simply TYPE those same three strings into an `ResearchAttemptAuthority`
// literal and pass -- REPAIR-02's negative tests only ever supplied a WRONG
// authority record; none proved a matching FABRICATED authority fails.
//
// DEFECT FIXED (REPAIR-04): REPAIR-03's judge lookup computed
// `sha256(serde_json::to_vec(&judge))` -- re-serializing the SUPPLIED judge
// JSON with Rust's own `serde_json` -- and looked for a
// `research_judge_artifacts` row whose `judge_artifact_sha256` equalled
// THAT hash. But Python's `json.dumps` (which produced the hash actually
// registered) and Rust's `serde_json` are not guaranteed to format every
// float identically: the SAME numeric value can be spelled `1e-06` by
// Python and `1e-6` by Rust. Two byte-different-but-semantically-identical
// judge artifacts therefore hashed to different values, and a genuinely
// authoritative, unmutated judge artifact could be falsely rejected. FIX:
// authority is no longer established by comparing a Rust-computed hash of
// the SUPPLIED JSON against the registry's stored hash. Instead, the
// registry now durably stores the exact canonical JSON TEXT Python produced
// (`canonical_judge_json`) alongside its own hash. This module (a) verifies
// each candidate row's OWN internal integrity --
// `sha256(canonical_judge_json) == judge_artifact_sha256` -- using ONLY
// bytes that originated in the registry itself, never the caller's, and (b)
// parses that trusted canonical text and the ACTUAL supplied judge JSON and
// compares them as PARSED VALUES, both re-serialized through the SAME
// (Rust-side) canonicalization -- never by trying to reproduce Python's
// exact formatting. This is invariant to float spelling, key ordering, and
// insignificant whitespace, while remaining fully sensitive to any genuine
// content difference (including a mutated DSR/PBO numeric OUTPUT). A row
// whose `canonical_judge_json` is missing (a historical row written before
// this repair) is never trusted, no matter what the caller supplies -- see
// [`find_matching_judge_row`].
//
// FIX (REPAIR-03, still current): this module is the ONLY source of a
// [`VerifiedResearchAuthority`], and it is not publicly constructible --
// every field is private and there is no `pub` constructor anywhere in this
// crate. The sole way to obtain one is [`load_research_authority`], which
// opens the durable Research registry (the SAME SQLite database
// `mqk_research.exp_distributed.storage.ResearchResultStore` writes:
// `research_trials`, `research_attempts`, and the additive
// `research_judge_artifacts` table) READ-ONLY, and itself establishes --
// from the registry's own rows, never from caller-supplied claims -- that:
//   1. `research_trials` contains `trial_id` (and yields its registered
//      `experiment_id`/`hypothesis_id`);
//   2. a `research_attempts` row for `trial_id` exists with
//      `status = 'succeeded'` and `result_id` equal to the supplied
//      `economic_eval_id` (the economic artifact's OWN
//      `ids.economic_eval_id`, extracted and verified elsewhere in
//      `verify_promotion_oos_evidence` before this is called);
//   3. a `research_judge_artifacts` row exists whose registered canonical
//      content (integrity-verified against its own `judge_artifact_sha256`)
//      is semantically identical to the ACTUAL supplied judge artifact, and
//      whose `experiment_id`/`hypothesis_id` belongs to this trial's own
//      registered experiment (a judge run scoped to a specific
//      `hypothesis_id` must match the trial's hypothesis_id exactly; a
//      judge run scoped to the whole experiment -- `hypothesis_id` is
//      `NULL`/absent -- covers every trial in that experiment).
//
// A caller cannot satisfy any of this by typing strings: it requires a real
// row, written by the real registry write path
// (`ResearchResultStore.register_trial` / `begin_attempt`+`finalize_attempt`
// / `ResearchResultStore.register_judge_artifact`, called from
// `mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge`
// once the judge's own DSR/PBO values are finalized -- see that function's
// own docs). This crate has NO write path into the registry and never
// creates the schema -- it only reads tables the Python registry already
// owns; `load_research_authority` fails closed if the database file is
// unreadable, or if any table/column it queries does not yet exist (a
// stale/wrong registry path, never silently treated as "no facts to
// check"). Fields are the confirmed facts, not currently read back by the
// caller (`verify_promotion_oos_evidence` already has
// `trial_id`/`economic_eval_id` from its own parsing) -- this is a witness
// type: its EXISTENCE (a successful `load_research_authority` call) is what
// matters, proving the three checks above all passed against real registry
// rows. Kept as a named struct, not `()`, so the registry-loading step
// stays independently testable and documented, per the mission's preferred
// design.
//
// TRUSTED PATH (mission Section 3F): `registry_db_path` is a plain function
// argument supplied by whichever code calls `verify_promotion_oos_evidence`.
// As of this repair there is no production call site anywhere in this
// workspace outside this crate's own tests -- the only callers are
// `tests/common` and the scenario test file, which each construct a fresh
// temporary registry. There is therefore no existing untrusted (e.g. HTTP
// request body) surface that could choose this path today; when a real
// caller is wired up, it must source `registry_db_path` from trusted
// application/deployment configuration, never from judge/economic-artifact
// JSON or other caller-supplied evidence metadata.
#[allow(dead_code)]
pub(crate) struct VerifiedResearchAuthority {
    pub(crate) trial_id: String,
    pub(crate) economic_eval_id: String,
    pub(crate) judge_artifact_sha256: String,
}

/// Open the registry read-only and establish authority for `trial_id`
/// against `economic_eval_id` (the economic artifact's own recorded
/// `ids.economic_eval_id`) and `supplied_judge` (the ACTUAL supplied judge
/// artifact, already parsed by the caller). Returns one human-readable
/// reason per failing check, same fail-closed contract as
/// [`crate::research_evidence::verify_promotion_oos_evidence`].
pub(crate) fn load_research_authority(
    registry_db_path: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    supplied_judge: &Value,
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

    // ---- 3. a registered judge artifact whose canonical content matches ----
    // (P7C-REPAIR-04: see module docs -- never a Rust-recomputed hash of
    // the supplied JSON compared against the registry's stored hash).
    let judge_match = find_matching_judge_row(&conn, supplied_judge, &mut errs);
    let mut registered_judge_sha256: Option<String> = None;
    match judge_match {
        None => {
            if !errs.iter().any(|e| e.contains("research_judge_artifacts")) {
                errs.push(
                    "OOS evidence rejected: research registry has no research_judge_artifacts \
                     row whose registered canonical judge content (independently verified \
                     against its own judge_artifact_sha256) matches the actual supplied judge \
                     artifact -- this judge artifact (including its DSR/PBO numeric output) was \
                     never durably registered"
                        .to_string(),
                );
            }
        }
        Some((judge_sha256, judge_experiment_id, judge_hypothesis_id)) => {
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
                     judge content belongs to experiment_id={judge_experiment_id:?} \
                     hypothesis_id={judge_hypothesis_id:?}, not trial_id {trial_id:?}'s own \
                     registered experiment_id={experiment_id:?} hypothesis_id={hypothesis_id:?} \
                     -- wrong experiment/hypothesis"
                ));
            } else {
                registered_judge_sha256 = Some(judge_sha256);
            }
        }
    }

    if !errs.is_empty() {
        return Err(errs);
    }

    Ok(VerifiedResearchAuthority {
        trial_id: trial_id.to_string(),
        economic_eval_id: economic_eval_id.to_string(),
        judge_artifact_sha256: registered_judge_sha256
            .expect("no errs means the judge row was matched and scope-verified above"),
    })
}

/// P7C-REPAIR-04: scans every `research_judge_artifacts` row looking for one
/// whose registered canonical content is semantically identical to
/// `supplied_judge`, and returns its `(judge_artifact_sha256, experiment_id,
/// hypothesis_id)` if found. Two independent checks gate trust in each row,
/// in order:
///
///   1. INTEGRITY: `canonical_judge_json` must be present (a historical row
///      written before this repair has none -- see `storage.py`'s additive
///      migration -- and is never trusted, no matter what the caller
///      supplies), and `sha256(canonical_judge_json.as_bytes())` must equal
///      that SAME row's own `judge_artifact_sha256` primary key. This uses
///      ONLY bytes that originated in the registry itself -- a row whose
///      stored text and stored hash disagree (corruption, or a direct
///      tamper of either column) is never trusted, even if its text happens
///      to otherwise look right.
///   2. CONTENT MATCH: the row's canonical text, parsed and re-serialized
///      through this crate's own `serde_json` (which sorts object keys --
///      no `preserve_order` feature anywhere in this workspace), must
///      produce BYTE-IDENTICAL output to `supplied_judge` re-serialized the
///      same way. Because both sides go through the SAME serializer, this
///      is invariant to float spelling (`1e-06` vs `1e-6` for the same
///      value), key ordering, and insignificant whitespace in the ORIGINAL
///      text, while remaining fully sensitive to any genuine value
///      difference.
///
/// `judge_artifact_sha256` is the primary key of `research_judge_artifacts`,
/// so at most one row can have canonical content matching any given target
/// (two different, integrity-verified rows can never share identical
/// content, since identical content hashes identically). Query failures
/// (missing table/column) are pushed into `errs` as fail-closed schema
/// errors, matching [`query_two_col_row`]/[`row_exists`]'s own contract.
fn find_matching_judge_row(
    conn: &Connection,
    supplied_judge: &Value,
    errs: &mut Vec<String>,
) -> Option<(String, String, Option<String>)> {
    let supplied_canonical_bytes = serde_json::to_vec(supplied_judge)
        .expect("a successfully-parsed serde_json::Value always re-serializes");

    let mut stmt = match conn.prepare(
        "select judge_artifact_sha256, experiment_id, hypothesis_id, canonical_judge_json \
         from research_judge_artifacts",
    ) {
        Ok(s) => s,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against research_judge_artifacts \
                 failed (missing/incompatible schema?): {e}"
            ));
            return None;
        }
    };

    let rows = match stmt.query_map([], |row| {
        let sha256: String = row.get(0)?;
        let experiment_id: String = row.get(1)?;
        let hypothesis_id: Option<String> = row.get(2)?;
        let canonical_judge_json: Option<String> = row.get(3)?;
        Ok((sha256, experiment_id, hypothesis_id, canonical_judge_json))
    }) {
        Ok(r) => r,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: research registry query against research_judge_artifacts \
                 failed: {e}"
            ));
            return None;
        }
    };

    for row in rows {
        let (sha256, experiment_id, hypothesis_id, canonical_judge_json) = match row {
            Ok(v) => v,
            Err(_) => continue,
        };
        // INTEGRITY check 1: a row lacking canonical text (historical, or a
        // future write bug) is never trusted -- no reconstruction/backfill
        // from caller input.
        let canonical_text = match canonical_judge_json {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        // INTEGRITY check 2: the row's own text must hash to its own
        // primary key.
        if sha256_hex(canonical_text.as_bytes()) != sha256 {
            continue;
        }
        let authoritative: Value = match serde_json::from_str(&canonical_text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let authoritative_bytes = serde_json::to_vec(&authoritative)
            .expect("a successfully-parsed serde_json::Value always re-serializes");
        if authoritative_bytes == supplied_canonical_bytes {
            return Some((sha256, experiment_id, hypothesis_id));
        }
    }
    None
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
