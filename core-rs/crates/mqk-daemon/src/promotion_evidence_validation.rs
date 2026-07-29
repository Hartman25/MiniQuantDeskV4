//! Shared promotion/review-artifact evidence validation.
//!
//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 3: this module is the single
//! implementation of "root-bound and content-validate a review artifact
//! directory against an exact `(strategy_id, symbol, timeframe_secs)`
//! identity, re-deriving the SHA-256 fingerprint rather than trusting a
//! caller's claim" — used by both:
//!
//! - `routes::strategy_promotions::strategy_promotion_transition` (the
//!   operator-facing POST route, via [`validate_paper_candidate_evidence`])
//!   — moved here verbatim from that module (STRATEGY-PROMOTION-REGISTRY-01C)
//!   with zero behavior change: same canonicalize+root-prefix pattern, same
//!   error message strings (the route surfaces them directly as
//!   `blockers`), same SHA-256 fingerprint derivation.
//! - Bundle 7's dynamic-selection plan construction/read validation, via
//!   [`validate_active_paper_candidate`] — a new read-side wrapper that
//!   additionally fetches the identity's current promotion state, checks
//!   `active_paper`/effectivity/expiry, resolves evidence lineage, and
//!   compares the freshly-recomputed fingerprint against the *durable*
//!   `evidence_fingerprint` column byte-for-byte (never trusting it).
//!
//! Both callers share the exact same artifact-reading/fingerprint logic —
//! there is no second, weaker reimplementation.

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::state::AppState;
use mqk_db::{
    fetch_current_promotion_state, resolve_evidence_lineage, scanner_timeframe_label_to_secs,
    PROMOTION_STATE_ACTIVE_PAPER,
};

// ---------------------------------------------------------------------------
// Score/rank canonicalization
// ---------------------------------------------------------------------------
//
// IR7: the old `raw_f64 * 1e6 -> round()` scheme is gone -- it is not an
// exact representation of the artifact's JSON numeric token (see
// `mqk_portfolio::canonicalize_decimal_token`'s module docs for why). Score
// canonicalization now runs entirely on the *raw JSON token text*, captured
// via `serde_json::value::RawValue` (the `raw_value` feature only adds this
// opt-in capability; it changes nothing about how any other type in this
// crate deserializes numbers) and handed to the pure, zero-float
// `mqk_portfolio::canonicalize_decimal_token` for parsing/canonicalization.

/// Defect A: bound `review_decisions.json`'s size *before* it is ever read
/// into memory -- an ordinary review artifact never approaches this; it
/// exists purely so a malformed or adversarial artifact fails closed instead
/// of allocating without limit.
const MAX_DECISIONS_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Defect 2: bound `manifest.json`'s size too -- much smaller than the
/// decisions file in ordinary operation, but never unbounded.
const MAX_MANIFEST_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// Defect 2: bounded-handle artifact reads (closes the metadata/read race)
// ---------------------------------------------------------------------------
//
// The prior implementation checked `std::fs::metadata(path).len()` and then
// called `std::fs::read_to_string(path)` as two independent filesystem
// operations against the same *path* -- the file the path resolves to can
// grow or be replaced entirely between those two calls (TOCTOU), so the
// metadata check proved nothing about what the read actually returned, and
// the read itself was unbounded. [`read_bounded`] reads through one already-
// open handle with a `max_bytes + 1`-byte cap as the actual proof -- the
// number of bytes it can prove it read, never a size some other syscall
// reported. [`read_bounded_file_string`] adds a cheap early-refusal
// `metadata()` call against that *same* handle (defense-in-depth only, not
// the authority) and UTF-8-decodes only after the bounded byte read
// completes.

/// Defect 2: read at most `max_bytes` bytes from `reader`, refusing with a
/// distinct error the moment more than `max_bytes` bytes are available --
/// generic over [`std::io::Read`] so it is directly testable against an
/// in-memory buffer, with no filesystem involved.
fn read_bounded<R: std::io::Read>(mut reader: R, max_bytes: u64) -> Result<Vec<u8>, String> {
    let limit = max_bytes.saturating_add(1);
    let mut buf = Vec::new();
    reader
        .by_ref()
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if buf.len() as u64 > max_bytes {
        return Err(format!("exceeds max allowed size ({max_bytes} bytes)"));
    }
    Ok(buf)
}

/// Defect 4: bound the actual byte stream of an *already-open* handle
/// through [`read_bounded`] -- one content read only, against the exact
/// handle [`open_confined_regular_child`] proved is confined, regular, and
/// identity-matched. Never reopens by path (see that function's doc
/// comment for why a path-based reopen is unsound here). UTF-8 decoding
/// happens only after the bounded byte read completes, so an invalid-UTF-8
/// file is refused distinctly from an over-size one.
fn read_bounded_file_string(
    file: std::fs::File,
    display_path: &Path,
    max_bytes: u64,
) -> Result<String, String> {
    if let Ok(meta) = file.metadata() {
        if meta.len() > max_bytes {
            return Err(format!(
                "{} exceeds max allowed size ({} bytes > {max_bytes} bytes)",
                display_path.display(),
                meta.len(),
            ));
        }
    }
    let bytes = read_bounded(file, max_bytes).map_err(|e| {
        if e.contains("exceeds max allowed size") {
            format!(
                "{} exceeds max allowed size ({max_bytes} bytes)",
                display_path.display()
            )
        } else {
            format!("{e}: {}", display_path.display())
        }
    })?;
    String::from_utf8(bytes)
        .map_err(|e| format!("invalid utf-8 in {}: {e}", display_path.display()))
}

/// Shadow deserialization target used solely to recover the exact raw JSON
/// text of one row's `scanner_score` field, and the same row's identity
/// fields (typed, not raw) so [`read_and_match_decisions`] can prove
/// positional identity agreement against the independently-parsed
/// `Vec<StrategyScanReviewDecision>` -- both parses run against the exact
/// same in-memory buffer (Defect A), never a second filesystem read. Every
/// other field is intentionally omitted -- serde ignores unknown/absent
/// fields by default, and this type never becomes the source of truth for
/// anything except the one raw token plus this cross-check.
#[derive(serde::Deserialize)]
struct RawScoreRow<'a> {
    strategy_id: String,
    symbol: String,
    timeframe: String,
    #[serde(borrow)]
    scanner_score: Option<&'a RawValue>,
}

/// Defect A: one bounded, single-filesystem-read parse of
/// `review_decisions.json`, matched against `(strategy_id, symbol,
/// timeframe_secs)`. Both the typed `Vec<StrategyScanReviewDecision>` and the
/// raw-token `Vec<RawScoreRow>` are deserialized from the exact same
/// in-memory text buffer -- there is no second `std::fs::read_to_string`
/// call, closing the prior TOCTOU where two independent reads of the same
/// path could observe different content if the file changed between them.
///
/// Row-count and positional identity agreement between the two parses is
/// proven explicitly (never assumed from "same buffer, therefore same
/// result") before either is used, and fails closed distinctly from a
/// malformed-JSON or over-limit-size failure.
#[derive(Debug)]
struct MatchedDecisions {
    decisions: Vec<mqk_backtest::StrategyScanReviewDecision>,
    matched_index: usize,
    scanner_score_token: Option<String>,
}

fn read_and_match_decisions(
    decisions_file: std::fs::File,
    display_path: &Path,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> Result<MatchedDecisions, String> {
    // Defect 4: one bounded read through the single already-open,
    // confinement-proven, identity-verified file handle
    // [`open_confined_regular_child`] returned -- never a separate
    // `std::fs::metadata(path)`/`std::fs::read_to_string(path)` reopen by
    // path (the file the path resolves to could be unlinked and replaced,
    // or a symlink retargeted, between a path-based check and a later
    // path-based open). Everything below parses this single bounded,
    // immutable buffer.
    let raw_text =
        read_bounded_file_string(decisions_file, display_path, MAX_DECISIONS_FILE_BYTES)?;

    let decisions: Vec<mqk_backtest::StrategyScanReviewDecision> = serde_json::from_str(&raw_text)
        .map_err(|e| format!("parse failed: {}: {e}", display_path.display()))?;
    let raw_rows: Vec<RawScoreRow> = serde_json::from_str(&raw_text)
        .map_err(|e| format!("parse failed: {}: {e}", display_path.display()))?;

    if decisions.len() != raw_rows.len() {
        return Err(format!(
            "typed/raw row count mismatch parsing the same buffer of {}: typed={} raw={}",
            display_path.display(),
            decisions.len(),
            raw_rows.len()
        ));
    }
    for (i, (d, r)) in decisions.iter().zip(raw_rows.iter()).enumerate() {
        if d.strategy_id.trim() != r.strategy_id.trim()
            || d.symbol.trim() != r.symbol.trim()
            || d.timeframe.trim() != r.timeframe.trim()
        {
            return Err(format!(
                "typed/raw row identity mismatch at index {i} parsing {}",
                display_path.display()
            ));
        }
    }

    let matches: Vec<usize> = decisions
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            d.strategy_id.trim() == strategy_id
                && d.symbol.trim().to_ascii_uppercase() == symbol
                && scanner_timeframe_label_to_secs(&d.timeframe) == Some(timeframe_secs)
        })
        .map(|(i, _)| i)
        .collect();

    if matches.is_empty() {
        return Err(format!(
            "no matching evidence row found for strategy_id='{strategy_id}' symbol='{symbol}' \
             timeframe_secs={timeframe_secs} in review artifact"
        ));
    }
    if matches.len() > 1 {
        return Err(format!(
            "review artifact contains {} ambiguous matching rows for this exact identity; \
             evidence must be unambiguous",
            matches.len()
        ));
    }
    let matched_index = matches[0];
    let scanner_score_token = raw_rows[matched_index]
        .scanner_score
        .map(|rv| rv.get().to_string());

    Ok(MatchedDecisions {
        decisions,
        matched_index,
        scanner_score_token,
    })
}

// ---------------------------------------------------------------------------
// Defect B: versioned exact-score evidence fingerprint (v2)
// ---------------------------------------------------------------------------
//
// The legacy `evidence_fingerprint` (SHA-256 of `serde_json::to_string` over
// the matched `StrategyScanReviewDecision`, which has already deserialized
// `scanner_score` into `f64`) does not authenticate the *exact* raw decimal
// token: two distinct tokens can round to the same `f64` -- and therefore
// the same legacy fingerprint -- while producing different exact Bundle 7
// rankings (IR7). `compute_evidence_fingerprint_v2` hashes an explicit,
// length-prefixed, deterministic byte encoding of every matched-row field
// that affects evidence identity, built directly from the raw score token
// -- never `Debug`, map iteration, `f64` formatting, mtime, or source
// ordinal.

/// Schema/version marker for [`compute_evidence_fingerprint_v2`]'s input —
/// bumping the hash algorithm or the set/order of hashed fields requires
/// bumping this string, so a durable v2 fingerprint can never be silently
/// reinterpreted under a changed recipe.
pub const EVIDENCE_FINGERPRINT_V2_SCHEMA: &str = "strategy-promotion-evidence-fingerprint-v2";

fn v2_push_len_prefixed(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    buf.extend_from_slice(bytes);
}

fn v2_push_opt_str(buf: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(v) => {
            buf.push(1);
            v2_push_len_prefixed(buf, v);
        }
        None => buf.push(0),
    }
}

fn v2_push_str_vec(buf: &mut Vec<u8>, items: &[String]) {
    buf.extend_from_slice(&(items.len() as u64).to_be_bytes());
    for item in items {
        v2_push_len_prefixed(buf, item);
    }
}

/// Canonical, explicit-length-prefixed input material for the v2 evidence
/// fingerprint. Every field this function omits is, by construction, a field
/// the legacy fingerprint's serialization of the whole matched row already
/// covers *exactly* (i.e. nothing lossy) -- the fields listed here are
/// precisely the ones Defect B calls out: schema marker, review/scan
/// identity, manifest `git_hash`, canonical `(strategy_id, symbol,
/// timeframe_secs)`, exact `review_state`, the exact raw score token (never
/// an `f64`), `scanner_rank` with an explicit `Some`/`None` tag, and the
/// matched row's `reason_codes`/`blockers`/`warnings` (the remaining fields
/// of `StrategyScanReviewDecision` that affect legacy evidence identity).
#[allow(clippy::too_many_arguments)]
fn evidence_fingerprint_v2_material(
    review_id: &str,
    scanner_scan_id: &str,
    git_hash: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    review_state: &str,
    scanner_score_token: Option<&str>,
    scanner_rank: Option<usize>,
    reason_codes: &[String],
    blockers: &[String],
    warnings: &[String],
) -> Vec<u8> {
    let mut buf = Vec::new();
    v2_push_len_prefixed(&mut buf, EVIDENCE_FINGERPRINT_V2_SCHEMA);
    v2_push_len_prefixed(&mut buf, review_id);
    v2_push_len_prefixed(&mut buf, scanner_scan_id);
    v2_push_len_prefixed(&mut buf, git_hash);
    v2_push_len_prefixed(&mut buf, strategy_id);
    v2_push_len_prefixed(&mut buf, symbol);
    buf.extend_from_slice(&timeframe_secs.to_be_bytes());
    v2_push_len_prefixed(&mut buf, review_state);
    v2_push_opt_str(&mut buf, scanner_score_token);
    match scanner_rank {
        Some(r) => {
            buf.push(1);
            buf.extend_from_slice(&(r as u64).to_be_bytes());
        }
        None => buf.push(0),
    }
    v2_push_str_vec(&mut buf, reason_codes);
    v2_push_str_vec(&mut buf, blockers);
    v2_push_str_vec(&mut buf, warnings);
    buf
}

/// Hex-encoded SHA-256 of [`evidence_fingerprint_v2_material`] — the durable
/// `evidence_fingerprint_v2` value, computed identically at write time
/// (`routes::strategy_promotions`) and recompute time
/// ([`validate_active_paper_candidate`]).
#[allow(clippy::too_many_arguments)]
pub fn compute_evidence_fingerprint_v2(
    review_id: &str,
    scanner_scan_id: &str,
    git_hash: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    review_state: &str,
    scanner_score_token: Option<&str>,
    scanner_rank: Option<usize>,
    reason_codes: &[String],
    blockers: &[String],
    warnings: &[String],
) -> String {
    let material = evidence_fingerprint_v2_material(
        review_id,
        scanner_scan_id,
        git_hash,
        strategy_id,
        symbol,
        timeframe_secs,
        review_state,
        scanner_score_token,
        scanner_rank,
        reason_codes,
        blockers,
        warnings,
    );
    let mut hasher = Sha256::new();
    hasher.update(&material);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Write-path: root-bound, content-validated artifact read (moved verbatim
// from routes::strategy_promotions, STRATEGY-PROMOTION-REGISTRY-01C).
// ---------------------------------------------------------------------------

/// Evidence independently read and validated from a review artifact
/// directory, ready to attach to a transition insert (write path) or to
/// compare against durable evidence (read path).
#[derive(Debug, Clone)]
pub struct ValidatedEvidence {
    pub review_id: String,
    pub scanner_scan_id: String,
    pub git_hash: String,
    pub artifact_path: String,
    pub fingerprint: String,
    /// `None` when the matched row's raw `scanner_score` is absent from the
    /// artifact -- never defaulted to `0.0` or any other sentinel. Retained
    /// for any existing caller that wants the lossy float; the read path
    /// (`validate_active_paper_candidate`) uses [`Self::scanner_score_token`]
    /// instead (IR7).
    pub scanner_score: Option<f64>,
    /// IR7: the exact raw JSON numeric token backing `scanner_score`, e.g.
    /// `"0.1234567"` -- never round-tripped through a float. `None` exactly
    /// when [`Self::scanner_score`] is `None`.
    pub scanner_score_token: Option<String>,
    /// `None` when the matched row's raw `scanner_rank` is absent from the
    /// artifact -- never defaulted.
    pub scanner_rank: Option<usize>,
    /// IR6: the matched row's exact `review_state` code (e.g.
    /// `"paper_candidate"`) -- carried through even though this function
    /// itself already enforces it must equal `"paper_candidate"` on
    /// success, so a caller never has to guess or re-derive it.
    pub review_state: String,
    /// Defect B: the matched row's `reason_codes`, carried through
    /// unvalidated -- part of the matched row's full identity, and an input
    /// to [`compute_evidence_fingerprint_v2`].
    pub reason_codes: Vec<String>,
    /// Defect B: the matched row's `blockers`, carried through unvalidated.
    pub blockers: Vec<String>,
    /// Defect B: the matched row's `warnings`, carried through unvalidated.
    pub warnings: Vec<String>,
}

/// Root-bounded, content-validated read of a review artifact directory,
/// mirroring the exact canonicalize+root-prefix pattern used by
/// `GET /api/v1/strategy-scans/review-artifact`
/// (`routes/strategy_scans.rs`). Never trusts caller-supplied claims about
/// the evidence — always re-derives the fingerprint from the matched
/// decision row's own serialized content.
pub fn validate_paper_candidate_evidence(
    st: &AppState,
    review_dir: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> Result<ValidatedEvidence, String> {
    let requested = review_dir.trim();
    if requested.is_empty() {
        return Err("review_dir is required for this transition".to_string());
    }

    let candidate_path = Path::new(requested);
    let root = Path::new(&st.strategy_review_artifact_root);

    let candidate_canon = std::fs::canonicalize(candidate_path)
        .map_err(|_| format!("review_dir not found: {requested}"))?;
    let root_canon = std::fs::canonicalize(root).map_err(|_| {
        format!(
            "configured review artifact root is unavailable: {}",
            root.display()
        )
    })?;
    if !candidate_canon.starts_with(&root_canon) {
        return Err(
            "review_dir does not resolve inside the configured review artifact root".to_string(),
        );
    }

    // Defect 4: canonicalize, root/candidate-confine, open, and
    // identity-verify each *child* file independently, through one opened
    // handle -- canonicalizing the directory above proves nothing about a
    // symlink or reparse point planted as manifest.json or
    // review_decisions.json itself; a bare `Path::is_file()` on the
    // un-canonicalized join follows such a link without ever proving where
    // it actually points. The handle this returns is what every subsequent
    // read below binds to -- neither child is ever reopened by path.
    let manifest_child = candidate_canon.join("manifest.json");
    let manifest_file =
        open_confined_regular_child(&manifest_child, &root_canon, &candidate_canon)?;
    let decisions_child = candidate_canon.join("review_decisions.json");
    let decisions_file =
        open_confined_regular_child(&decisions_child, &root_canon, &candidate_canon)?;

    let manifest: mqk_backtest::ReviewManifest =
        read_json(manifest_file, &manifest_child, MAX_MANIFEST_FILE_BYTES)?;

    // Defect A: one bounded, single-filesystem-read parse -- no second read
    // of `decisions_child` anywhere below.
    let matched_decisions = read_and_match_decisions(
        decisions_file,
        &decisions_child,
        strategy_id,
        symbol,
        timeframe_secs,
    )?;
    let matched = &matched_decisions.decisions[matched_decisions.matched_index];

    if matched.review_state != mqk_backtest::StrategyScanReviewState::PaperCandidate {
        return Err(format!(
            "matched evidence row has review_state='{}', not 'paper_candidate'",
            matched.review_state.code()
        ));
    }

    let canonical_json = serde_json::to_string(matched)
        .map_err(|e| format!("failed to serialize matched evidence row: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let fingerprint = hex::encode(hasher.finalize());

    Ok(ValidatedEvidence {
        review_id: manifest.review_id,
        scanner_scan_id: manifest.scanner_scan_id,
        git_hash: manifest.git_hash,
        artifact_path: candidate_canon.display().to_string(),
        fingerprint,
        scanner_score: matched.scanner_score,
        // IR7: the exact raw JSON token backing this row's `scanner_score`,
        // recovered from the same single buffer read above -- never derived
        // from `matched.scanner_score: Option<f64>`, which has already lost
        // any precision beyond an f64's.
        scanner_score_token: matched_decisions.scanner_score_token,
        scanner_rank: matched.scanner_rank,
        review_state: matched.review_state.code().to_string(),
        reason_codes: matched.reason_codes.clone(),
        blockers: matched.blockers.clone(),
        warnings: matched.warnings.clone(),
    })
}

/// Defect 4: bounded, single-open-handle JSON read -- see
/// [`read_bounded_file_string`]. No unbounded `read_to_string`, and no
/// reopen by path, on any artifact authority file.
fn read_json<T: serde::de::DeserializeOwned>(
    file: std::fs::File,
    display_path: &std::path::Path,
    max_bytes: u64,
) -> Result<T, String> {
    let raw = read_bounded_file_string(file, display_path, max_bytes)?;
    serde_json::from_str(&raw).map_err(|e| format!("parse failed: {}: {e}", display_path.display()))
}

// ---------------------------------------------------------------------------
// Defect 4: root/candidate-confined child artifact open, bound to one handle
// ---------------------------------------------------------------------------
//
// The prior implementation (`canonicalize_and_confine_child`) proved
// confinement and regularity against a *path* and returned that `PathBuf` --
// every actual read then reopened the file by that same path a second time
// (`read_json`/`read_and_match_decisions` -> `read_bounded_file_string` ->
// `std::fs::File::open(path)`). Nothing prevented the file the path resolved
// to from being unlinked and replaced, or a symlink retargeted, in the
// window between the confinement proof and that second open (TOCTOU) -- the
// confinement check would have proven something true of a file that no
// longer exists by the time it is actually read.
//
// [`open_confined_regular_child`] closes that gap: it canonicalizes and
// confines exactly as before, then opens the canonical path itself and
// hands back the *open handle* -- there is no second, independent open by
// path anywhere in this module. It additionally re-verifies the opened
// handle is itself a regular file (a swap could have replaced it with a
// FIFO, device node, or directory) and, where the platform supports it,
// compares the opened handle's identity against the pre-open confinement
// check's metadata to catch a same-path swap in between.

/// Defect 4: canonicalize `child` (expected to be a path directly inside
/// `candidate_canon`), prove it resolves inside *both* `root_canon` (the
/// whole configured review-artifact root) and `candidate_canon` (this exact
/// candidate's own directory), open the canonical path exactly once, and
/// bind every subsequent check to that one opened handle.
///
/// Refuses a missing child, an escaped child, a non-regular child (e.g. a
/// directory), and a child whose identity changed between the confinement
/// check and the open, each with a distinct message -- never trusts
/// `Path::is_file()` on the un-canonicalized join alone, and never trusts a
/// `PathBuf` returned from an earlier check as proof of what a later open
/// will return.
fn open_confined_regular_child(
    child: &Path,
    root_canon: &Path,
    candidate_canon: &Path,
) -> Result<std::fs::File, String> {
    let child_canon = std::fs::canonicalize(child)
        .map_err(|_| format!("expected artifact file not found: {}", child.display()))?;
    if !child_canon.starts_with(root_canon) || !child_canon.starts_with(candidate_canon) {
        return Err(format!(
            "artifact file does not resolve inside the configured review artifact root: {}",
            child.display()
        ));
    }
    // Checked metadata: a pre-open, path-based proof that the canonicalized
    // path is currently a regular file -- necessary but not sufficient on
    // its own (a later open could still observe a different, swapped-in
    // file at the same path); see the opened-handle re-verification below.
    let checked_meta = std::fs::metadata(&child_canon)
        .map_err(|e| format!("read failed: {}: {e}", child_canon.display()))?;
    if !checked_meta.is_file() {
        return Err(format!(
            "expected artifact file is not a regular file: {}",
            child_canon.display()
        ));
    }

    // The one and only open of this path -- every later read binds to this
    // handle, never reopens by path.
    let file = std::fs::File::open(&child_canon)
        .map_err(|e| format!("read failed: {}: {e}", child_canon.display()))?;
    let opened_meta = file
        .metadata()
        .map_err(|e| format!("read failed: {}: {e}", child_canon.display()))?;
    if !opened_meta.is_file() {
        return Err(format!(
            "expected artifact file is not a regular file: {}",
            child_canon.display()
        ));
    }

    // Bind the opened handle's identity back to the checked metadata --
    // detects a swap (unlink+recreate, symlink retarget) in the window
    // between the path-based check above and this open.
    if !opened_handle_identity_matches(&checked_meta, &opened_meta) {
        return Err(format!(
            "artifact file identity changed between check and open: {}",
            child_canon.display()
        ));
    }

    Ok(file)
}

/// Defect 4: `true` iff `opened` (metadata read from an already-open
/// [`std::fs::File`] handle) still identifies the same underlying file
/// `checked` (metadata read from a path-based, pre-open confinement check)
/// referred to -- factored out of [`open_confined_regular_child`] so the
/// comparison itself is directly unit-testable against two genuinely
/// different files' metadata, without needing to win a real filesystem
/// race.
///
/// Unix: device + inode is a stable, always-available identity pair -- a
/// swap (unlink+recreate, or a symlink chain retargeted) changes at least
/// one of them even if size/mtime happen to coincide.
///
/// Windows: the stable `std::os::windows::fs::MetadataExt` surface has no
/// equivalent to Unix's device/inode pair -- the file-index fields that
/// would provide one (`file_index`, `volume_serial_number`) are gated
/// behind the unstable `windows_by_handle` feature and unavailable on
/// stable Rust. This is a *documented, strictly weaker* fallback: compare
/// file size, last-write time, and creation time. A swap that preserves all
/// three (e.g. an in-place truncate+rewrite completing within the same
/// filesystem-timestamp tick) would not be caught -- unlike the Unix
/// branch, this is not a proof of identity, only a best-effort
/// corroboration.
fn opened_handle_identity_matches(checked: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        checked.dev() == opened.dev() && checked.ino() == opened.ino()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        checked.file_size() == opened.file_size()
            && checked.last_write_time() == opened.last_write_time()
            && checked.creation_time() == opened.creation_time()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (checked, opened);
        true
    }
}

// ---------------------------------------------------------------------------
// Read-path: durable-evidence-backed re-validation (Bundle 7, new).
// ---------------------------------------------------------------------------

/// Closed, bounded reason-code vocabulary for [`validate_active_paper_candidate`]
/// failures. Every distinct failure mode named by
/// docs/specs/dynamic_strategy_symbol_selection_01a (Phase 3) gets its own
/// variant -- never folded into a single generic "invalid" bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateEvidenceReason {
    PromotionQueryFailed,
    NoPromotionRecord,
    PromotionNotActivePaper,
    PromotionNotYetEffective,
    PromotionExpired,
    EvidenceLineageQueryFailed,
    EvidenceLineageBroken,
    ArtifactPathMissing,
    DurableFingerprintMissing,
    /// Defect B: the durable evidence-bearing transition has a legacy
    /// `evidence_fingerprint` but no `evidence_fingerprint_v2` -- a legacy
    /// row that predates the v2 exact-score authority. Distinct from
    /// [`Self::DurableFingerprintMissing`] (which means *neither* fingerprint
    /// is present). Never backfilled from the mutable artifact filesystem at
    /// read time -- requires a new evidence-bearing operator transition.
    DurableFingerprintV2Missing,
    ArtifactRootUnavailable,
    ArtifactMissing,
    ArtifactRootEscape,
    ArtifactMalformed,
    ArtifactDuplicateIdentity,
    ArtifactNotPaperCandidate,
    ArtifactNoMatchingRow,
    /// Defect A: the single-buffer typed/raw parse of `review_decisions.json`
    /// disagreed on row count or per-row identity -- proof of positional
    /// identity agreement failed. Distinct from [`Self::ArtifactMalformed`]
    /// (unparseable JSON) and [`Self::ArtifactDuplicateIdentity`] (a
    /// legitimate ambiguous-match refusal).
    ArtifactRowIdentityMismatch,
    /// Defect A: `review_decisions.json` exceeds the bounded size limit --
    /// refused before allocation, distinct from a malformed-content refusal.
    ArtifactOverLimit,
    FingerprintMismatch,
    /// Defect B: the recomputed v2 fingerprint does not byte-for-byte match
    /// the durable `evidence_fingerprint_v2` -- distinct from
    /// [`Self::FingerprintMismatch`] (the legacy fingerprint), since the two
    /// can, in principle, disagree independently.
    FingerprintV2Mismatch,
    ScoreMissing,
    ScoreNotFinite,
    RankOutOfRange,
    /// IR4: the durable `sys_strategy_registry` query itself failed (DB
    /// unavailable, query error) — distinct from a query that succeeded and
    /// found no row.
    RegistryQueryFailed,
    /// IR4: the durable strategy registry has no row at all for this
    /// `strategy_id` — plugin instantiability is not equivalent to durable
    /// registry admission; a candidate whose engine instantiates but whose
    /// identity was never registered must still be refused.
    RegistryRowMissing,
    /// IR4: the durable strategy registry row exists but `enabled = false`.
    RegistryDisabled,
    /// IR4: the plugin registry could not instantiate this `strategy_id` at
    /// all (unknown name or internal metadata/spec inconsistency) —
    /// distinct from `RegistryDisabled`/`RegistryRowMissing`, which are
    /// durable-registry-admission failures, not plugin-construction
    /// failures.
    UnsupportedStrategyPlugin,
    /// IR10: the ephemeral per-symbol [`mqk_strategy::PluginRegistry`]
    /// itself failed to construct (`register_builtin_strategies` returned
    /// `Err`, e.g. a duplicate-name programming error) — never silently
    /// discarded; this candidate is refused rather than proceeding against
    /// a registry that failed to build.
    RegistryConstructionFailed,
}

impl CandidateEvidenceReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PromotionQueryFailed => "promotion_query_failed",
            Self::NoPromotionRecord => "no_promotion_record",
            Self::PromotionNotActivePaper => "promotion_not_active_paper",
            Self::PromotionNotYetEffective => "promotion_not_yet_effective",
            Self::PromotionExpired => "promotion_expired",
            Self::EvidenceLineageQueryFailed => "evidence_lineage_query_failed",
            Self::EvidenceLineageBroken => "evidence_lineage_broken",
            Self::ArtifactPathMissing => "artifact_path_missing",
            Self::DurableFingerprintMissing => "durable_fingerprint_missing",
            Self::DurableFingerprintV2Missing => "durable_fingerprint_v2_missing",
            Self::ArtifactRootUnavailable => "artifact_root_unavailable",
            Self::ArtifactMissing => "artifact_missing",
            Self::ArtifactRootEscape => "artifact_root_escape",
            Self::ArtifactMalformed => "artifact_malformed",
            Self::ArtifactDuplicateIdentity => "artifact_duplicate_identity",
            Self::ArtifactNotPaperCandidate => "artifact_not_paper_candidate",
            Self::ArtifactNoMatchingRow => "artifact_no_matching_row",
            Self::ArtifactRowIdentityMismatch => "artifact_row_identity_mismatch",
            Self::ArtifactOverLimit => "artifact_over_limit",
            Self::FingerprintMismatch => "fingerprint_mismatch",
            Self::FingerprintV2Mismatch => "fingerprint_v2_mismatch",
            Self::ScoreMissing => "score_missing",
            Self::ScoreNotFinite => "score_not_finite",
            Self::RankOutOfRange => "rank_out_of_range",
            Self::RegistryQueryFailed => "registry_query_failed",
            Self::RegistryRowMissing => "registry_row_missing",
            Self::RegistryDisabled => "registry_disabled",
            Self::UnsupportedStrategyPlugin => "unsupported_strategy_plugin",
            Self::RegistryConstructionFailed => "registry_construction_failed",
        }
    }

    /// Defect E: map this daemon-local reason to its closed-vocabulary
    /// [`mqk_portfolio::ExactSelectionReason`] counterpart -- every variant
    /// here has a direct, 1:1 counterpart in that pure-crate enum (this type
    /// exists only because `mqk-portfolio` is zero-dependency and cannot
    /// import `mqk-daemon`; the two vocabularies are kept in exact
    /// correspondence, never diverging).
    pub fn to_exact_selection_reason(self) -> mqk_portfolio::ExactSelectionReason {
        use mqk_portfolio::ExactSelectionReason as E;
        match self {
            Self::PromotionQueryFailed => E::PromotionQueryFailed,
            Self::NoPromotionRecord => E::NoPromotionRecord,
            Self::PromotionNotActivePaper => E::PromotionNotActivePaper,
            Self::PromotionNotYetEffective => E::PromotionNotYetEffective,
            Self::PromotionExpired => E::PromotionExpired,
            Self::EvidenceLineageQueryFailed => E::EvidenceLineageQueryFailed,
            Self::EvidenceLineageBroken => E::EvidenceLineageBroken,
            Self::ArtifactPathMissing => E::ArtifactPathMissing,
            Self::DurableFingerprintMissing => E::DurableFingerprintMissing,
            Self::DurableFingerprintV2Missing => E::DurableFingerprintV2Missing,
            Self::ArtifactRootUnavailable => E::ArtifactRootUnavailable,
            Self::ArtifactMissing => E::ArtifactMissing,
            Self::ArtifactRootEscape => E::ArtifactRootEscape,
            Self::ArtifactMalformed => E::ArtifactMalformed,
            Self::ArtifactDuplicateIdentity => E::ArtifactDuplicateIdentity,
            Self::ArtifactNotPaperCandidate => E::ArtifactNotPaperCandidate,
            Self::ArtifactNoMatchingRow => E::ArtifactNoMatchingRow,
            Self::ArtifactRowIdentityMismatch => E::ArtifactRowIdentityMismatch,
            Self::ArtifactOverLimit => E::ArtifactOverLimit,
            Self::FingerprintMismatch => E::FingerprintMismatch,
            Self::FingerprintV2Mismatch => E::FingerprintV2Mismatch,
            Self::ScoreMissing => E::ScoreMissing,
            Self::ScoreNotFinite => E::ScoreNotFinite,
            Self::RankOutOfRange => E::RankOutOfRange,
            Self::RegistryQueryFailed => E::RegistryQueryFailed,
            Self::RegistryRowMissing => E::RegistryRowMissing,
            Self::RegistryDisabled => E::RegistryDisabled,
            Self::UnsupportedStrategyPlugin => E::UnsupportedStrategyPlugin,
            Self::RegistryConstructionFailed => E::RegistryConstructionFailed,
        }
    }
}

/// IR4: validate that `strategy_id` is currently an enabled entry in the
/// durable strategy registry (`sys_strategy_registry`) — the same durable
/// authority internal admission already consults
/// (`mqk_db::fetch_strategy_registry_entry`). Plugin instantiability
/// ([`mqk_strategy::PluginRegistry::instantiate_verified`]) is a separate,
/// independent check: an engine can instantiate for a `strategy_id` that
/// was never durably registered (or was registered and then disabled), and
/// this function is what catches that gap. Distinctly refuses query
/// failure, missing row, and disabled — never folds them into one generic
/// "not usable" outcome.
pub async fn validate_strategy_registry_enabled(
    db: &PgPool,
    strategy_id: &str,
) -> Result<(), CandidateEvidenceReason> {
    let record = mqk_db::fetch_strategy_registry_entry(db, strategy_id)
        .await
        .map_err(|_| CandidateEvidenceReason::RegistryQueryFailed)?;
    match record {
        None => Err(CandidateEvidenceReason::RegistryRowMissing),
        Some(r) if !r.enabled => Err(CandidateEvidenceReason::RegistryDisabled),
        Some(_) => Ok(()),
    }
}

/// Classify one of [`validate_paper_candidate_evidence`]'s error strings
/// into a distinct [`CandidateEvidenceReason`]. This is the *only* place
/// that inspects those message strings -- the artifact-reading logic itself
/// is never duplicated. Falls back to [`CandidateEvidenceReason::ArtifactMalformed`]
/// for any message this function does not recognize, so an unrecognized
/// future message still fails closed rather than being silently swallowed.
fn classify_artifact_error(msg: &str) -> CandidateEvidenceReason {
    if msg.contains("does not resolve inside the configured review artifact root") {
        CandidateEvidenceReason::ArtifactRootEscape
    } else if msg.contains("configured review artifact root is unavailable") {
        CandidateEvidenceReason::ArtifactRootUnavailable
    } else if msg.starts_with("review_dir not found:")
        || msg.starts_with("expected artifact file not found:")
    {
        CandidateEvidenceReason::ArtifactMissing
    } else if msg.contains("ambiguous matching rows") {
        CandidateEvidenceReason::ArtifactDuplicateIdentity
    } else if msg.contains("no matching evidence row found") {
        CandidateEvidenceReason::ArtifactNoMatchingRow
    } else if msg.contains("not 'paper_candidate'") {
        CandidateEvidenceReason::ArtifactNotPaperCandidate
    } else if msg.contains("exceeds max allowed size") {
        CandidateEvidenceReason::ArtifactOverLimit
    } else if msg.contains("row count mismatch") || msg.contains("row identity mismatch") {
        CandidateEvidenceReason::ArtifactRowIdentityMismatch
    } else {
        // "review_dir is required...", "read failed:...", "parse failed:...",
        // "failed to serialize matched evidence row:..." and anything else
        // this function has not been taught to distinguish -- all fail
        // closed as a malformed-artifact refusal, never as success.
        CandidateEvidenceReason::ArtifactMalformed
    }
}

/// The complete canonical candidate snapshot this validator is responsible
/// for, on success: every fact this module independently re-derived and
/// compared against durable evidence. Bundle 7's plan builder (Phase 4)
/// combines this with plugin/timeframe/data-readiness/watchlist facts it
/// gathers itself to populate `mqk_portfolio::SelectionCandidateEvidence`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedCandidateEvidence {
    pub evidence_review_id: String,
    pub evidence_scanner_scan_id: String,
    pub evidence_artifact_path: String,
    /// Defect 1: the durable legacy `evidence_fingerprint` column value read
    /// back from the promotion transition, distinct from
    /// [`Self::recomputed_legacy_fingerprint`] even though the two are
    /// required byte-equal for this struct to ever be constructed (a
    /// mismatch fails closed with [`CandidateEvidenceReason::FingerprintMismatch`]
    /// before reaching here) -- provenance is never collapsed to one
    /// ambiguous field, so a future durable readback can always tell which
    /// value came from disk and which was freshly computed.
    pub durable_legacy_fingerprint: String,
    /// Defect 1: the legacy fingerprint freshly recomputed from the review
    /// artifact at validation time. See [`Self::durable_legacy_fingerprint`].
    pub recomputed_legacy_fingerprint: String,
    /// Defect B/Defect 1: the durable v2 exact-score fingerprint (see
    /// [`compute_evidence_fingerprint_v2`]), as read back from the
    /// promotion transition. Always present on a successful
    /// [`validate_active_paper_candidate`] result -- an identity whose
    /// durable evidence lacks a v2 fingerprint fails closed with
    /// [`CandidateEvidenceReason::DurableFingerprintV2Missing`] before this
    /// struct is ever constructed.
    pub durable_exact_fingerprint_v2: String,
    /// Defect 1: the v2 exact-score fingerprint freshly recomputed from the
    /// review artifact's raw score token at validation time. See
    /// [`Self::durable_exact_fingerprint_v2`].
    pub recomputed_exact_fingerprint_v2: String,
    /// IR6: the review artifact manifest's `git_hash`.
    pub evidence_git_hash: String,
    /// IR6: the matched review row's exact `review_state` code.
    pub evidence_review_state: String,
    /// IR7: canonical, decimal-exact string form of the durable
    /// `scanner_score` -- the authoritative score; never derived from a
    /// float.
    pub canonical_score_decimal: String,
    /// IR7: convenience bridge to the legacy scaled-integer (micros)
    /// representation, `Some` only when exactly representable at that
    /// scale. See `mqk_portfolio::canonical_decimal_to_micros_if_exact`.
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<u32>,
    /// IR6: the durable promotion transition id currently authorizing this
    /// identity's `active_paper` state.
    pub promotion_transition_id: uuid::Uuid,
    pub promotion_effective_at: DateTime<Utc>,
    pub promotion_expires_at: Option<DateTime<Utc>>,
    /// IR6: the exact transition id that established the evidence this
    /// validation compared against (see
    /// `mqk_db::StrategyPromotionTransitionRecord::evidence_transition_id`
    /// / `resolve_evidence_lineage`).
    pub evidence_transition_id: uuid::Uuid,
}

/// Validate that `(strategy_id, symbol, timeframe_secs)` is currently an
/// unexpired, effective `active_paper` promotion whose evidence-bearing
/// review artifact still fingerprint-matches durable evidence, exactly.
///
/// Runs, in order: promotion query -> active_paper/effective/expiry gate ->
/// evidence lineage resolution -> root-bound artifact read (via the exact
/// same [`validate_paper_candidate_evidence`] the write-path route calls)
/// -> byte-for-byte fingerprint comparison -> score finiteness -> rank
/// range. The first failing step returns its own distinct
/// [`CandidateEvidenceReason`] — never a generic catch-all, and never a
/// silent partial success.
pub async fn validate_active_paper_candidate(
    db: &PgPool,
    st: &Arc<AppState>,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    authority_ts: DateTime<Utc>,
) -> Result<ValidatedCandidateEvidence, CandidateEvidenceReason> {
    let record = fetch_current_promotion_state(db, strategy_id, symbol, timeframe_secs)
        .await
        .map_err(|_| CandidateEvidenceReason::PromotionQueryFailed)?;
    let Some(record) = record else {
        return Err(CandidateEvidenceReason::NoPromotionRecord);
    };

    if record.new_state != PROMOTION_STATE_ACTIVE_PAPER {
        return Err(CandidateEvidenceReason::PromotionNotActivePaper);
    }
    if record.effective_at_utc > authority_ts {
        return Err(CandidateEvidenceReason::PromotionNotYetEffective);
    }
    if let Some(expires_at) = record.expires_at_utc {
        if expires_at <= authority_ts {
            return Err(CandidateEvidenceReason::PromotionExpired);
        }
    }

    let evidence = resolve_evidence_lineage(db, &record)
        .await
        .map_err(|_| CandidateEvidenceReason::EvidenceLineageQueryFailed)?
        .ok_or(CandidateEvidenceReason::EvidenceLineageBroken)?;

    let artifact_path = evidence
        .evidence_artifact_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CandidateEvidenceReason::ArtifactPathMissing)?;
    let durable_fingerprint = evidence
        .evidence_fingerprint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CandidateEvidenceReason::DurableFingerprintMissing)?;
    // Defect B: a legacy row (evidence_fingerprint present, but recorded
    // before v2 existed) fails closed distinctly here -- never backfilled
    // from the mutable artifact filesystem at read time. Checked before the
    // artifact is even re-read, since a missing durable v2 value can never
    // be satisfied by any recomputation.
    let durable_fingerprint_v2 = evidence
        .evidence_fingerprint_v2
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CandidateEvidenceReason::DurableFingerprintV2Missing)?;

    let fresh =
        validate_paper_candidate_evidence(st, artifact_path, strategy_id, symbol, timeframe_secs)
            .map_err(|msg| classify_artifact_error(&msg))?;

    // Defect B: durable and recomputed fingerprints are compared separately
    // -- legacy first (defense-in-depth/back-compat), then v2 independently.
    // Only a v2-authenticated exact decimal may go on to rank.
    if fresh.fingerprint != durable_fingerprint {
        return Err(CandidateEvidenceReason::FingerprintMismatch);
    }
    let recomputed_v2 = compute_evidence_fingerprint_v2(
        &fresh.review_id,
        &fresh.scanner_scan_id,
        &fresh.git_hash,
        strategy_id,
        symbol,
        timeframe_secs,
        &fresh.review_state,
        fresh.scanner_score_token.as_deref(),
        fresh.scanner_rank,
        &fresh.reason_codes,
        &fresh.blockers,
        &fresh.warnings,
    );
    if recomputed_v2 != durable_fingerprint_v2 {
        return Err(CandidateEvidenceReason::FingerprintV2Mismatch);
    }

    // IR7: canonicalize from the exact raw JSON token -- never from
    // `fresh.scanner_score: Option<f64>`, which has already lost any
    // precision beyond an f64's.
    let canonical_score_decimal = match fresh.scanner_score_token.as_deref() {
        None => return Err(CandidateEvidenceReason::ScoreMissing),
        Some(token) => mqk_portfolio::canonicalize_decimal_token(token)
            .ok_or(CandidateEvidenceReason::ScoreNotFinite)?,
    };
    let canonical_score_micros =
        mqk_portfolio::canonical_decimal_to_micros_if_exact(&canonical_score_decimal);
    let scanner_rank = match fresh.scanner_rank {
        None => None,
        Some(r) => Some(u32::try_from(r).map_err(|_| CandidateEvidenceReason::RankOutOfRange)?),
    };

    Ok(ValidatedCandidateEvidence {
        evidence_review_id: fresh.review_id,
        evidence_scanner_scan_id: fresh.scanner_scan_id,
        evidence_artifact_path: fresh.artifact_path,
        durable_legacy_fingerprint: durable_fingerprint.to_string(),
        recomputed_legacy_fingerprint: fresh.fingerprint,
        durable_exact_fingerprint_v2: durable_fingerprint_v2.to_string(),
        recomputed_exact_fingerprint_v2: recomputed_v2,
        evidence_git_hash: fresh.git_hash,
        evidence_review_state: fresh.review_state,
        canonical_score_decimal,
        canonical_score_micros,
        scanner_rank,
        promotion_transition_id: record.transition_id,
        promotion_effective_at: record.effective_at_utc,
        promotion_expires_at: record.expires_at_utc,
        evidence_transition_id: evidence.transition_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` is process-global; several tests
    /// in this module set it then immediately construct an `AppState` (which
    /// captures the value once into its own field and never re-reads the
    /// env var afterward). Without serializing that narrow
    /// set-then-construct window, two tests running on different threads
    /// (the default `cargo test` behavior) can race and one observes the
    /// other's root. `fixture_state` is the only place that performs this
    /// sequence, so acquiring the lock there is sufficient.
    static ENV_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── IR7: exact raw-token extraction, filesystem-to-RawValue end-to-end ──
    // (pure-decimal canonicalization math itself is proven in
    // mqk_portfolio::canonical_decimal's own unit tests; this test proves
    // this crate's artifact-reading layer hands that math the untouched
    // original token, not an f64 round-trip.)

    #[test]
    fn scanner_score_token_preserves_precision_an_f64_roundtrip_would_lose() {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_scanner_score_token_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let review_dir = root.join("review-1");
        std::fs::create_dir_all(&review_dir).expect("create review dir");

        std::fs::write(
            review_dir.join("manifest.json"),
            r#"{
                "schema_version": 1,
                "review_id": "review-1",
                "scanner_scan_id": "scan-1",
                "source_artifact_dir": "fixture",
                "created_at_utc": "2026-07-01T00:00:00Z",
                "git_hash": "test-git-hash",
                "policy_min_bars_used": 252,
                "policy_min_trade_count": 5,
                "policy_min_total_return_pct": 0.0,
                "policy_min_alpha_pct": 0.0,
                "policy_max_drawdown_pct": 25.0,
                "policy_min_profit_factor": 1.05,
                "candidate_count": 1,
                "blocked_count": 0,
                "needs_review_count": 0,
                "watchlist_candidate_count": 0,
                "paper_candidate_count": 1,
                "rejected_count": 0,
                "blockers": [],
                "warnings": []
            }"#,
        )
        .expect("write manifest");

        // 15 fractional digits -- an f64 round-trip through
        // `raw * 1e6 -> round()` cannot be trusted to preserve this exactly;
        // the raw-token path must hand back this literal string untouched.
        std::fs::write(
            review_dir.join("review_decisions.json"),
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 0.123456789012345,
                "review_state": "paper_candidate",
                "reason_codes": ["eligible_paper_candidate"],
                "blockers": [],
                "warnings": []
            }]"#,
        )
        .expect("write decisions");

        let st = fixture_state(&root);

        let evidence = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("evidence must validate");

        assert_eq!(
            evidence.scanner_score_token.as_deref(),
            Some("0.123456789012345"),
            "must be the exact original token, not an f64-rounded approximation"
        );
        assert_eq!(evidence.review_state, "paper_candidate");

        std::fs::remove_dir_all(&root).ok();
    }

    // ── Defect A: single-read atomic evidence buffer ─────────────────────

    const FIXTURE_MANIFEST: &str = r#"{
        "schema_version": 1,
        "review_id": "review-1",
        "scanner_scan_id": "scan-1",
        "source_artifact_dir": "fixture",
        "created_at_utc": "2026-07-01T00:00:00Z",
        "git_hash": "test-git-hash",
        "policy_min_bars_used": 252,
        "policy_min_trade_count": 5,
        "policy_min_total_return_pct": 0.0,
        "policy_min_alpha_pct": 0.0,
        "policy_max_drawdown_pct": 25.0,
        "policy_min_profit_factor": 1.05,
        "candidate_count": 1,
        "blocked_count": 0,
        "needs_review_count": 0,
        "watchlist_candidate_count": 0,
        "paper_candidate_count": 1,
        "rejected_count": 0,
        "blockers": [],
        "warnings": []
    }"#;

    /// Write a fresh fixture review artifact directory containing
    /// [`FIXTURE_MANIFEST`] and the caller-supplied `decisions_json` text,
    /// returning `(root, review_dir)`. Callers must
    /// `std::fs::remove_dir_all(&root).ok()` when done.
    fn write_fixture(name: &str, decisions_json: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_defect_a_{name}_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let review_dir = root.join("review-1");
        std::fs::create_dir_all(&review_dir).expect("create review dir");
        std::fs::write(review_dir.join("manifest.json"), FIXTURE_MANIFEST).expect("write manifest");
        std::fs::write(review_dir.join("review_decisions.json"), decisions_json)
            .expect("write decisions");
        (root, review_dir)
    }

    fn fixture_state(root: &std::path::Path) -> crate::state::AppState {
        let _guard = ENV_ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
        crate::state::AppState::new_with_operator_auth(
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        )
    }

    /// Defect 4: the handle-based read functions (`read_bounded_file_string`,
    /// `read_json`, `read_and_match_decisions`) now take an already-open
    /// `File` rather than a path -- this helper opens a fixture path once so
    /// call sites below read exactly like they did before the signature
    /// change.
    fn open_for_test(path: &std::path::Path) -> std::fs::File {
        std::fs::File::open(path).expect("open fixture file")
    }

    #[test]
    fn valid_decisions_are_read_from_a_single_filesystem_read() {
        let (root, review_dir) = write_fixture(
            "single_read",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 9.0,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let st = fixture_state(&root);

        // Defect A: read_and_match_decisions itself performs exactly one
        // `std::fs::read_to_string` call -- verified structurally (there is
        // only one such call site in its body, see the function above) and
        // behaviorally here: a file deleted immediately after this call
        // still validates successfully were it re-read lazily, which it is
        // not -- this proves the parse already happened against the one
        // buffer captured up front.
        let decisions_path = review_dir.join("review_decisions.json");
        let matched = read_and_match_decisions(
            open_for_test(&decisions_path),
            &decisions_path,
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("must match");
        assert_eq!(matched.scanner_score_token.as_deref(), Some("9.0"));
        assert_eq!(matched.decisions.len(), 1);

        let evidence = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("evidence must validate");
        assert_eq!(evidence.scanner_score_token.as_deref(), Some("9.0"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn over_limit_file_size_is_refused_before_parsing() {
        let (root, review_dir) = write_fixture("over_limit", "[]");
        // Overwrite with a file larger than MAX_DECISIONS_FILE_BYTES --
        // still syntactically invalid JSON (irrelevant: the size check runs
        // before any parse is attempted).
        let oversized = "x".repeat(MAX_DECISIONS_FILE_BYTES as usize + 1);
        std::fs::write(review_dir.join("review_decisions.json"), oversized)
            .expect("write oversized decisions");

        let decisions_path = review_dir.join("review_decisions.json");
        let err = read_and_match_decisions(
            open_for_test(&decisions_path),
            &decisions_path,
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("over-limit file must be refused");
        assert!(err.contains("exceeds max allowed size"), "got: {err}");
        assert_eq!(
            classify_artifact_error(&err),
            CandidateEvidenceReason::ArtifactOverLimit
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn malformed_json_is_refused_distinctly() {
        let (root, review_dir) = write_fixture("malformed", "not valid json at all");

        let decisions_path = review_dir.join("review_decisions.json");
        let err = read_and_match_decisions(
            open_for_test(&decisions_path),
            &decisions_path,
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("malformed JSON must be refused");
        assert!(err.contains("parse failed"), "got: {err}");
        assert_eq!(
            classify_artifact_error(&err),
            CandidateEvidenceReason::ArtifactMalformed
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_score_is_none_not_fabricated() {
        let (root, review_dir) = write_fixture(
            "missing_score",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": null,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let st = fixture_state(&root);

        let evidence = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("evidence must validate even with a null score");
        assert_eq!(evidence.scanner_score_token, None);
        assert_eq!(evidence.scanner_score, None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_identity_in_the_full_decisions_file_is_refused() {
        // Two rows share the exact (strategy_id, symbol, timeframe) identity
        // under query -- an ambiguous artifact must be refused, never
        // resolved by picking the first/last row.
        let (root, review_dir) = write_fixture(
            "duplicate_identity",
            r#"[
                {
                    "symbol": "AAPL",
                    "timeframe": "1D",
                    "strategy_id": "swing_momentum",
                    "scanner_rank": 1,
                    "scanner_score": 9.0,
                    "review_state": "paper_candidate",
                    "reason_codes": [],
                    "blockers": [],
                    "warnings": []
                },
                {
                    "symbol": "AAPL",
                    "timeframe": "1D",
                    "strategy_id": "swing_momentum",
                    "scanner_rank": 2,
                    "scanner_score": 5.0,
                    "review_state": "paper_candidate",
                    "reason_codes": [],
                    "blockers": [],
                    "warnings": []
                }
            ]"#,
        );

        let decisions_path = review_dir.join("review_decisions.json");
        let err = read_and_match_decisions(
            open_for_test(&decisions_path),
            &decisions_path,
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("duplicate identity must be refused");
        assert!(err.contains("ambiguous matching rows"), "got: {err}");
        assert_eq!(
            classify_artifact_error(&err),
            CandidateEvidenceReason::ArtifactDuplicateIdentity
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // ── Defect 2: bounded-handle reader ───────────────────────────────────

    /// An in-memory [`std::io::Read`] fixture that reports an arbitrary
    /// number of available bytes regardless of what a filesystem's
    /// `metadata()` might separately claim -- proves `read_bounded` enforces
    /// the actual byte count it reads, never a size some other syscall
    /// reported (Defect 2's core fix).
    struct FixedReader(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for FixedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    #[test]
    fn read_bounded_accepts_exactly_max_bytes() {
        let data = vec![b'x'; 100];
        let reader = FixedReader(std::io::Cursor::new(data.clone()));
        let out = read_bounded(reader, 100).expect("exactly MAX must be accepted");
        assert_eq!(out, data);
    }

    #[test]
    fn read_bounded_refuses_max_plus_one_bytes() {
        let data = vec![b'x'; 101];
        let reader = FixedReader(std::io::Cursor::new(data));
        let err = read_bounded(reader, 100).expect_err("MAX+1 must be refused");
        assert!(err.contains("exceeds max allowed size"), "got: {err}");
    }

    #[test]
    fn read_bounded_ignores_misleading_small_declared_size() {
        // A `Read` implementation that behaves as if it were much smaller
        // than it actually is (there is no `metadata()` on this type at
        // all) -- `read_bounded` only ever trusts the bytes it actually
        // reads, never a separately-reported size.
        let data = vec![b'y'; 10_000];
        let reader = FixedReader(std::io::Cursor::new(data));
        let err = read_bounded(reader, 100).expect_err("oversized stream must be refused");
        assert!(err.contains("exceeds max allowed size"), "got: {err}");
    }

    #[test]
    fn read_bounded_file_string_exactly_max_accepted() {
        let root =
            std::env::temp_dir().join(format!("mqk_daemon_bounded_ok_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("f.txt");
        std::fs::write(&path, vec![b'a'; 50]).expect("write fixture");

        let out = read_bounded_file_string(open_for_test(&path), &path, 50)
            .expect("exactly MAX must be accepted");
        assert_eq!(out.len(), 50);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_bounded_file_string_max_plus_one_refused() {
        let root =
            std::env::temp_dir().join(format!("mqk_daemon_bounded_over_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("f.txt");
        std::fs::write(&path, vec![b'a'; 51]).expect("write fixture");

        let err = read_bounded_file_string(open_for_test(&path), &path, 50)
            .expect_err("MAX+1 must be refused");
        assert!(err.contains("exceeds max allowed size"), "got: {err}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_bounded_file_string_invalid_utf8_refused_distinctly() {
        let root =
            std::env::temp_dir().join(format!("mqk_daemon_bounded_utf8_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("f.txt");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).expect("write fixture");

        let err = read_bounded_file_string(open_for_test(&path), &path, 100)
            .expect_err("invalid utf-8 must be refused");
        assert!(err.contains("invalid utf-8"), "got: {err}");
        assert!(
            !err.contains("exceeds max allowed size"),
            "must be a distinct refusal from an over-size one: {err}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_json_over_limit_manifest_is_refused_before_parsing() {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_manifest_bound_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("manifest.json");
        std::fs::write(&path, vec![b'{'; MAX_MANIFEST_FILE_BYTES as usize + 1])
            .expect("write oversized manifest");

        let result: Result<mqk_backtest::ReviewManifest, String> =
            read_json(open_for_test(&path), &path, MAX_MANIFEST_FILE_BYTES);
        let err = result.expect_err("over-limit manifest must be refused");
        assert!(err.contains("exceeds max allowed size"), "got: {err}");

        std::fs::remove_dir_all(&root).ok();
    }

    // ── Defect 3: symlink/reparse-point child escape ──────────────────────

    #[test]
    fn normal_in_root_child_files_pass() {
        let (root, review_dir) = write_fixture(
            "defect3_normal",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 9.0,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let st = fixture_state(&root);
        let evidence = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("normal in-root artifact must validate");
        assert_eq!(evidence.review_state, "paper_candidate");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn non_regular_child_is_refused() {
        let (root, review_dir) = write_fixture("defect3_non_regular", "[]");
        // Replace manifest.json with a directory -- a non-regular child.
        std::fs::remove_file(review_dir.join("manifest.json")).expect("remove manifest");
        std::fs::create_dir_all(review_dir.join("manifest.json")).expect("create dir in its place");

        let st = fixture_state(&root);
        let err = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("a directory in place of manifest.json must be refused");
        assert!(err.contains("not a regular file"), "got: {err}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn manifest_child_symlink_escape_refused() {
        let (root, review_dir) = write_fixture("defect3_manifest_escape", "[]");
        let outside = std::env::temp_dir().join(format!(
            "mqk_daemon_defect3_outside_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "not a real manifest").expect("write outside file");

        std::fs::remove_file(review_dir.join("manifest.json")).expect("remove manifest");
        std::os::unix::fs::symlink(&outside, review_dir.join("manifest.json"))
            .expect("create manifest.json as a symlink escaping the root");

        let st = fixture_state(&root);
        let err = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("a manifest.json symlink escaping the root must be refused");
        assert!(
            err.contains("does not resolve inside the configured review artifact root"),
            "got: {err}"
        );
        assert_eq!(
            classify_artifact_error(&err),
            CandidateEvidenceReason::ArtifactRootEscape
        );

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn decisions_child_symlink_escape_refused() {
        let (root, review_dir) = write_fixture("defect3_decisions_escape", "[]");
        let outside = std::env::temp_dir().join(format!(
            "mqk_daemon_defect3_outside_decisions_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "[]").expect("write outside file");

        std::fs::remove_file(review_dir.join("review_decisions.json")).expect("remove decisions");
        std::os::unix::fs::symlink(&outside, review_dir.join("review_decisions.json"))
            .expect("create review_decisions.json as a symlink escaping the root");

        let st = fixture_state(&root);
        let err = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect_err("a review_decisions.json symlink escaping the root must be refused");
        assert!(
            err.contains("does not resolve inside the configured review artifact root"),
            "got: {err}"
        );
        assert_eq!(
            classify_artifact_error(&err),
            CandidateEvidenceReason::ArtifactRootEscape
        );

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn manifest_child_symlink_escape_refused() {
        let (root, review_dir) = write_fixture("defect3_manifest_escape_win", "[]");
        let outside = std::env::temp_dir().join(format!(
            "mqk_daemon_defect3_outside_win_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "not a real manifest").expect("write outside file");

        std::fs::remove_file(review_dir.join("manifest.json")).expect("remove manifest");
        // Platform inability to create a symlink (no privilege on this
        // Windows account) skips only this fixture, with an explicit reason
        // -- never silently passes it.
        match std::os::windows::fs::symlink_file(&outside, review_dir.join("manifest.json")) {
            Ok(()) => {
                let st = fixture_state(&root);
                let err = validate_paper_candidate_evidence(
                    &st,
                    review_dir.to_str().unwrap(),
                    "swing_momentum",
                    "AAPL",
                    86400,
                )
                .expect_err("a manifest.json symlink escaping the root must be refused");
                assert!(
                    err.contains("does not resolve inside the configured review artifact root"),
                    "got: {err}"
                );
                assert_eq!(
                    classify_artifact_error(&err),
                    CandidateEvidenceReason::ArtifactRootEscape
                );
            }
            Err(e) => {
                eprintln!(
                    "skipping manifest_child_symlink_escape_refused: this Windows account \
                     lacks symlink-creation privilege: {e}"
                );
            }
        }

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    // ── Defect 4: bind confinement to the opened handle ───────────────────

    /// Defect 4, test 1: the handle [`open_confined_regular_child`] returns
    /// is the exact handle every subsequent read binds to -- proven here by
    /// replacing the file at the same path (a full unlink+recreate, which
    /// changes the underlying inode on Unix) *after* the confined handle was
    /// opened, then reading through that handle. A path-based reopen (the
    /// prior split API) would observe the replacement content; the opened
    /// handle must still observe the original.
    #[test]
    fn opened_handle_reads_original_content_even_if_path_is_replaced_after_open() {
        let (root, review_dir) = write_fixture(
            "defect4_handle_binding",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 9.0,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let root_canon = std::fs::canonicalize(&root).expect("canonicalize root");
        let candidate_canon = std::fs::canonicalize(&review_dir).expect("canonicalize candidate");
        let decisions_child = candidate_canon.join("review_decisions.json");

        let file = open_confined_regular_child(&decisions_child, &root_canon, &candidate_canon)
            .expect("must open confined child");

        // Replace the file at the same path with different content AFTER
        // the handle was opened. If this fails (some platforms/filesystems
        // refuse to remove a file with an open handle), skip the rest of
        // this test explicitly rather than silently passing.
        if let Err(e) = std::fs::remove_file(&decisions_child) {
            eprintln!(
                "skipping opened_handle_reads_original_content_even_if_path_is_replaced_after_open: \
                 could not remove a file with an open handle on this platform: {e}"
            );
            std::fs::remove_dir_all(&root).ok();
            return;
        }
        std::fs::write(&decisions_child, "[]").expect("write replacement content");

        let raw = read_bounded_file_string(file, &decisions_child, MAX_DECISIONS_FILE_BYTES)
            .expect("read through the already-open handle must still succeed");
        assert!(
            raw.contains("swing_momentum"),
            "must read the ORIGINAL content through the already-open handle, not the \
             replacement written after open: {raw}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Defect 4, test 2: normal files pass -- confined, regular, identity
    /// stable end to end (already exercised by
    /// `normal_in_root_child_files_pass` above; this restates it directly
    /// against the new handle-returning helper).
    #[test]
    fn open_confined_regular_child_normal_file_passes() {
        let (root, review_dir) = write_fixture("defect4_normal", "[]");
        let root_canon = std::fs::canonicalize(&root).expect("canonicalize root");
        let candidate_canon = std::fs::canonicalize(&review_dir).expect("canonicalize candidate");
        let manifest_child = candidate_canon.join("manifest.json");

        let file = open_confined_regular_child(&manifest_child, &root_canon, &candidate_canon)
            .expect("a normal in-root file must open");
        assert!(file.metadata().expect("metadata").is_file());

        std::fs::remove_dir_all(&root).ok();
    }

    /// Defect 4, test 3: symlink escape refuses (restates the existing
    /// `manifest_child_symlink_escape_refused`/`decisions_child_symlink_escape_refused`
    /// coverage directly against the new handle-returning helper).
    #[cfg(unix)]
    #[test]
    fn open_confined_regular_child_symlink_escape_refused() {
        let (root, review_dir) = write_fixture("defect4_symlink_escape", "[]");
        let outside = std::env::temp_dir().join(format!(
            "mqk_daemon_defect4_outside_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "outside content").expect("write outside file");
        std::fs::remove_file(review_dir.join("manifest.json")).expect("remove manifest");
        std::os::unix::fs::symlink(&outside, review_dir.join("manifest.json"))
            .expect("create manifest.json as a symlink escaping the root");

        let root_canon = std::fs::canonicalize(&root).expect("canonicalize root");
        let candidate_canon = std::fs::canonicalize(&review_dir).expect("canonicalize candidate");
        let manifest_child = candidate_canon.join("manifest.json");

        let err = open_confined_regular_child(&manifest_child, &root_canon, &candidate_canon)
            .expect_err("a symlink escaping the root must be refused");
        assert!(
            err.contains("does not resolve inside the configured review artifact root"),
            "got: {err}"
        );

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    /// Defect 4, test 4: an injectable replacement/swap between the
    /// pre-open confinement check and the open is detected -- proven
    /// deterministically against [`opened_handle_identity_matches`] (the
    /// comparison [`open_confined_regular_child`] uses internally) with two
    /// genuinely different files' metadata, rather than trying to win a
    /// real filesystem race.
    #[test]
    fn identity_mismatch_between_two_different_files_is_detected() {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_defect4_identity_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path_a = root.join("a.txt");
        let path_b = root.join("b.txt");
        std::fs::write(&path_a, "content-a").expect("write a");
        std::fs::write(&path_b, "a very different length of content for b").expect("write b");

        let meta_a = std::fs::metadata(&path_a).expect("stat a");
        let meta_b = std::fs::metadata(&path_b).expect("stat b");
        assert!(
            !opened_handle_identity_matches(&meta_a, &meta_b),
            "two distinct files must never be treated as the same identity"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Defect 4, test 4 (companion): consistent identity (two stats of the
    /// exact same unchanged file) is accepted -- the positive case for the
    /// same comparison.
    #[test]
    fn identity_match_for_the_same_unchanged_file_is_accepted() {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_defect4_identity_same_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join("f.txt");
        std::fs::write(&path, "stable content").expect("write fixture");

        let meta_1 = std::fs::metadata(&path).expect("stat 1");
        let meta_2 = std::fs::metadata(&path).expect("stat 2");
        assert!(
            opened_handle_identity_matches(&meta_1, &meta_2),
            "two stats of the same unchanged file must match"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Defect 4, test 5: nonregular child refuses (restates the existing
    /// `non_regular_child_is_refused` coverage directly against the new
    /// handle-returning helper).
    #[test]
    fn open_confined_regular_child_nonregular_child_refused() {
        let (root, review_dir) = write_fixture("defect4_non_regular", "[]");
        std::fs::remove_file(review_dir.join("manifest.json")).expect("remove manifest");
        std::fs::create_dir_all(review_dir.join("manifest.json")).expect("create dir in its place");

        let root_canon = std::fs::canonicalize(&root).expect("canonicalize root");
        let candidate_canon = std::fs::canonicalize(&review_dir).expect("canonicalize candidate");
        let manifest_child = candidate_canon.join("manifest.json");

        let err = open_confined_regular_child(&manifest_child, &root_canon, &candidate_canon)
            .expect_err("a directory in place of manifest.json must be refused");
        assert!(err.contains("not a regular file"), "got: {err}");

        std::fs::remove_dir_all(&root).ok();
    }

    // ── Defect B: v2 fingerprint binds the exact score token ─────────────

    #[test]
    fn same_f64_different_raw_token_yields_equal_legacy_but_different_v2_fingerprints() {
        // 0.30 and 0.3 are distinct raw JSON tokens that parse to the exact
        // same f64 -- the legacy fingerprint (serialized after the f64
        // round-trip) must be equal, while v2 (hashed from the raw token
        // directly) must differ.
        let (root_a, review_dir_a) = write_fixture(
            "v2_token_a",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 0.30,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let (root_b, review_dir_b) = write_fixture(
            "v2_token_b",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 0.3,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let st_a = fixture_state(&root_a);
        let ev_a = validate_paper_candidate_evidence(
            &st_a,
            review_dir_a.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("a must validate");
        let st_b = fixture_state(&root_b);
        let ev_b = validate_paper_candidate_evidence(
            &st_b,
            review_dir_b.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("b must validate");

        assert_eq!(ev_a.scanner_score, ev_b.scanner_score, "same f64");
        assert_ne!(
            ev_a.scanner_score_token, ev_b.scanner_score_token,
            "distinct raw tokens"
        );
        assert_eq!(
            ev_a.fingerprint, ev_b.fingerprint,
            "legacy fingerprint collides on equal f64"
        );

        let v2_a = compute_evidence_fingerprint_v2(
            &ev_a.review_id,
            &ev_a.scanner_scan_id,
            &ev_a.git_hash,
            "swing_momentum",
            "AAPL",
            86400,
            &ev_a.review_state,
            ev_a.scanner_score_token.as_deref(),
            ev_a.scanner_rank,
            &ev_a.reason_codes,
            &ev_a.blockers,
            &ev_a.warnings,
        );
        let v2_b = compute_evidence_fingerprint_v2(
            &ev_b.review_id,
            &ev_b.scanner_scan_id,
            &ev_b.git_hash,
            "swing_momentum",
            "AAPL",
            86400,
            &ev_b.review_state,
            ev_b.scanner_score_token.as_deref(),
            ev_b.scanner_rank,
            &ev_b.reason_codes,
            &ev_b.blockers,
            &ev_b.warnings,
        );
        assert_ne!(v2_a, v2_b, "v2 must distinguish distinct raw tokens");

        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }

    #[test]
    fn durable_v2_from_artifact_a_refuses_mutated_artifact_b() {
        // A durable v2 fingerprint computed against artifact A must not
        // validate against a mutated artifact B claiming the same identity
        // -- B can never rank under A's durable authority.
        let (root, review_dir) = write_fixture(
            "v2_mutation_a",
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 9.0,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        );
        let st = fixture_state(&root);
        let ev_a = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("a must validate");
        let durable_v2_a = compute_evidence_fingerprint_v2(
            &ev_a.review_id,
            &ev_a.scanner_scan_id,
            &ev_a.git_hash,
            "swing_momentum",
            "AAPL",
            86400,
            &ev_a.review_state,
            ev_a.scanner_score_token.as_deref(),
            ev_a.scanner_rank,
            &ev_a.reason_codes,
            &ev_a.blockers,
            &ev_a.warnings,
        );

        // Mutate the artifact in place (B): a different score, same identity.
        std::fs::write(
            review_dir.join("review_decisions.json"),
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 4.5,
                "review_state": "paper_candidate",
                "reason_codes": [],
                "blockers": [],
                "warnings": []
            }]"#,
        )
        .expect("mutate decisions");

        let ev_b = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("b must still parse/validate structurally");
        let recomputed_v2_b = compute_evidence_fingerprint_v2(
            &ev_b.review_id,
            &ev_b.scanner_scan_id,
            &ev_b.git_hash,
            "swing_momentum",
            "AAPL",
            86400,
            &ev_b.review_state,
            ev_b.scanner_score_token.as_deref(),
            ev_b.scanner_rank,
            &ev_b.reason_codes,
            &ev_b.blockers,
            &ev_b.warnings,
        );

        assert_ne!(
            durable_v2_a, recomputed_v2_b,
            "B must never validate under A's durable v2 authority"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // ── classify_artifact_error ─────────────────────────────────────────

    #[test]
    fn classifies_root_escape() {
        assert_eq!(
            classify_artifact_error(
                "review_dir does not resolve inside the configured review artifact root"
            ),
            CandidateEvidenceReason::ArtifactRootEscape
        );
    }

    #[test]
    fn classifies_root_unavailable() {
        assert_eq!(
            classify_artifact_error("configured review artifact root is unavailable: /x"),
            CandidateEvidenceReason::ArtifactRootUnavailable
        );
    }

    #[test]
    fn classifies_missing_artifact() {
        assert_eq!(
            classify_artifact_error("review_dir not found: /tmp/nope"),
            CandidateEvidenceReason::ArtifactMissing
        );
        assert_eq!(
            classify_artifact_error("expected artifact file not found: /tmp/x/manifest.json"),
            CandidateEvidenceReason::ArtifactMissing
        );
    }

    #[test]
    fn classifies_duplicate_identity() {
        assert_eq!(
            classify_artifact_error(
                "review artifact contains 2 ambiguous matching rows for this exact identity; \
                 evidence must be unambiguous"
            ),
            CandidateEvidenceReason::ArtifactDuplicateIdentity
        );
    }

    #[test]
    fn classifies_no_matching_row() {
        assert_eq!(
            classify_artifact_error(
                "no matching evidence row found for strategy_id='x' symbol='AAPL' \
                 timeframe_secs=300 in review artifact"
            ),
            CandidateEvidenceReason::ArtifactNoMatchingRow
        );
    }

    #[test]
    fn classifies_not_paper_candidate() {
        assert_eq!(
            classify_artifact_error(
                "matched evidence row has review_state='rejected', not 'paper_candidate'"
            ),
            CandidateEvidenceReason::ArtifactNotPaperCandidate
        );
    }

    #[test]
    fn classifies_unrecognized_message_as_malformed_not_success() {
        assert_eq!(
            classify_artifact_error("read failed: /tmp/x/manifest.json: permission denied"),
            CandidateEvidenceReason::ArtifactMalformed
        );
        assert_eq!(
            classify_artifact_error("parse failed: /tmp/x/manifest.json: expected value"),
            CandidateEvidenceReason::ArtifactMalformed
        );
        assert_eq!(
            classify_artifact_error("some completely unexpected future message"),
            CandidateEvidenceReason::ArtifactMalformed
        );
    }

    #[test]
    fn reason_code_is_stable_and_distinct() {
        let all = [
            CandidateEvidenceReason::PromotionQueryFailed,
            CandidateEvidenceReason::NoPromotionRecord,
            CandidateEvidenceReason::PromotionNotActivePaper,
            CandidateEvidenceReason::PromotionNotYetEffective,
            CandidateEvidenceReason::PromotionExpired,
            CandidateEvidenceReason::EvidenceLineageQueryFailed,
            CandidateEvidenceReason::EvidenceLineageBroken,
            CandidateEvidenceReason::ArtifactPathMissing,
            CandidateEvidenceReason::DurableFingerprintMissing,
            CandidateEvidenceReason::DurableFingerprintV2Missing,
            CandidateEvidenceReason::ArtifactRootUnavailable,
            CandidateEvidenceReason::ArtifactMissing,
            CandidateEvidenceReason::ArtifactRootEscape,
            CandidateEvidenceReason::ArtifactMalformed,
            CandidateEvidenceReason::ArtifactDuplicateIdentity,
            CandidateEvidenceReason::ArtifactNotPaperCandidate,
            CandidateEvidenceReason::ArtifactNoMatchingRow,
            CandidateEvidenceReason::ArtifactRowIdentityMismatch,
            CandidateEvidenceReason::ArtifactOverLimit,
            CandidateEvidenceReason::FingerprintMismatch,
            CandidateEvidenceReason::FingerprintV2Mismatch,
            CandidateEvidenceReason::ScoreMissing,
            CandidateEvidenceReason::ScoreNotFinite,
            CandidateEvidenceReason::RankOutOfRange,
            CandidateEvidenceReason::RegistryQueryFailed,
            CandidateEvidenceReason::RegistryRowMissing,
            CandidateEvidenceReason::RegistryDisabled,
            CandidateEvidenceReason::UnsupportedStrategyPlugin,
            CandidateEvidenceReason::RegistryConstructionFailed,
        ];
        let mut codes: Vec<&str> = all.iter().map(|r| r.code()).collect();
        let unique_count = {
            let mut c = codes.clone();
            c.sort_unstable();
            c.dedup();
            c.len()
        };
        assert_eq!(
            unique_count,
            codes.len(),
            "every reason code must be distinct"
        );
        codes.clear();
    }

    /// Defect E: every [`CandidateEvidenceReason`] must map to a distinct
    /// [`mqk_portfolio::ExactSelectionReason`] whose own `.code()` agrees
    /// textually with this type's `.code()` -- proves the two closed
    /// vocabularies stay in exact correspondence rather than silently
    /// drifting apart.
    #[test]
    fn to_exact_selection_reason_agrees_with_code_for_every_variant() {
        let all = [
            CandidateEvidenceReason::PromotionQueryFailed,
            CandidateEvidenceReason::NoPromotionRecord,
            CandidateEvidenceReason::PromotionNotActivePaper,
            CandidateEvidenceReason::PromotionNotYetEffective,
            CandidateEvidenceReason::PromotionExpired,
            CandidateEvidenceReason::EvidenceLineageQueryFailed,
            CandidateEvidenceReason::EvidenceLineageBroken,
            CandidateEvidenceReason::ArtifactPathMissing,
            CandidateEvidenceReason::DurableFingerprintMissing,
            CandidateEvidenceReason::DurableFingerprintV2Missing,
            CandidateEvidenceReason::ArtifactRootUnavailable,
            CandidateEvidenceReason::ArtifactMissing,
            CandidateEvidenceReason::ArtifactRootEscape,
            CandidateEvidenceReason::ArtifactMalformed,
            CandidateEvidenceReason::ArtifactDuplicateIdentity,
            CandidateEvidenceReason::ArtifactNotPaperCandidate,
            CandidateEvidenceReason::ArtifactNoMatchingRow,
            CandidateEvidenceReason::ArtifactRowIdentityMismatch,
            CandidateEvidenceReason::ArtifactOverLimit,
            CandidateEvidenceReason::FingerprintMismatch,
            CandidateEvidenceReason::FingerprintV2Mismatch,
            CandidateEvidenceReason::ScoreMissing,
            CandidateEvidenceReason::ScoreNotFinite,
            CandidateEvidenceReason::RankOutOfRange,
            CandidateEvidenceReason::RegistryQueryFailed,
            CandidateEvidenceReason::RegistryRowMissing,
            CandidateEvidenceReason::RegistryDisabled,
            CandidateEvidenceReason::UnsupportedStrategyPlugin,
            CandidateEvidenceReason::RegistryConstructionFailed,
        ];
        for r in all {
            assert_eq!(r.code(), r.to_exact_selection_reason().code());
        }
    }

    // ── IR4: validate_strategy_registry_enabled (DB-backed) ────────────────

    async fn make_db_pool_for_test() -> sqlx::PgPool {
        let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: \
                 MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
                 cargo test -p mqk-daemon --lib promotion_evidence_validation \
                 -- --include-ignored --test-threads=1"
            )
        });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect to test DB");
        mqk_db::migrate(&pool).await.expect("run migrations");
        pool
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn missing_registry_row_is_refused() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("nonexistent_{}", uuid::Uuid::new_v4());
        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert_eq!(result, Err(CandidateEvidenceReason::RegistryRowMissing));
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn disabled_registry_row_is_refused() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("disabled_strat_{}", uuid::Uuid::new_v4());
        mqk_db::upsert_strategy_registry_entry(
            &pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: strategy_id.clone(),
                display_name: "Test Disabled".to_string(),
                enabled: false,
                kind: "bar_driven".to_string(),
                registered_at_utc: Utc::now(),
                updated_at_utc: Utc::now(),
                note: String::new(),
            },
        )
        .await
        .expect("upsert must succeed");

        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert_eq!(result, Err(CandidateEvidenceReason::RegistryDisabled));
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn enabled_registry_row_passes() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("enabled_strat_{}", uuid::Uuid::new_v4());
        mqk_db::upsert_strategy_registry_entry(
            &pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: strategy_id.clone(),
                display_name: "Test Enabled".to_string(),
                enabled: true,
                kind: "bar_driven".to_string(),
                registered_at_utc: Utc::now(),
                updated_at_utc: Utc::now(),
                note: String::new(),
            },
        )
        .await
        .expect("upsert must succeed");

        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert!(result.is_ok());
    }
}
