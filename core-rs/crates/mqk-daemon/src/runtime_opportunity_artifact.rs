//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase B — daemon-side (read-only)
//! `runtime-opportunity-set-v1` artifact loader/validator.
//!
//! Mirrors `watchlist_intake.rs`'s pattern: a pure evaluator over
//! already-read file contents, producing a closed outcome enum, every
//! validation failure accumulated rather than short-circuiting. The daemon
//! never writes this artifact — it is produced by the research-py
//! scanner/promotion pipeline
//! (`research-py/src/mqk_research/scanner/runtime_opportunity_artifact.py`)
//! and consumed read-only here.
//!
//! Phase A audit (docs/specs/runtime_opportunity_allocation_01a) established
//! this artifact is the *only* source-proven score the allocator may use —
//! the daemon's `LoadedWatchlistArtifact` carries no score at all.

use std::path::Path;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::watchlist_intake::{LoadedWatchlistArtifact, MULTI_SYMBOL_HARD_CEILING};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const ENV_RUNTIME_OPPORTUNITY_SET_PATH: &str = "MQK_RUNTIME_OPPORTUNITY_SET_PATH";
pub const SCHEMA_VERSION: &str = "runtime-opportunity-set-v1";
pub const SCORE_SOURCE_SCANNER_TOTAL_SCORE: &str = "scanner_total_score";

// ---------------------------------------------------------------------------
// Canonical watchlist lineage hash (must match
// research-py/.../runtime_opportunity_artifact.py::watchlist_lineage_hash
// byte-for-byte; see that function's docstring for why this is a
// delimiter-based canonical string rather than JSON serialization)
// ---------------------------------------------------------------------------

/// Recompute the watchlist lineage hash from the raw, already-parsed
/// watchlist JSON `serde_json::Value` (not the stripped
/// [`LoadedWatchlistArtifact`], which does not carry `generated_at_utc`).
pub fn watchlist_lineage_hash(watchlist_json: &serde_json::Value) -> String {
    let schema_version = watchlist_json
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let generated_at_utc = watchlist_json
        .get("generated_at_utc")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let max_symbols_to_trade = watchlist_json
        .get("max_symbols_to_trade")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let max_concurrent_positions = watchlist_json
        .get("max_concurrent_positions")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let symbols: Vec<String> = watchlist_json
        .get("symbols")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let symbols_str = symbols.join(",");

    let mut sa_pairs: Vec<(String, String)> = watchlist_json
        .get("strategy_assignments")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    sa_pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let sa_str = sa_pairs
        .iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(",");

    let blob = format!(
        "schema_version={schema_version}|generated_at_utc={generated_at_utc}|\
         max_symbols_to_trade={max_symbols_to_trade}|\
         max_concurrent_positions={max_concurrent_positions}|symbols={symbols_str}|\
         strategy_assignments={sa_str}"
    );

    let mut hasher = Sha256::new();
    hasher.update(blob.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Loaded types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedRuntimeOpportunityCandidate {
    pub symbol: String,
    pub strategy_id: String,
    /// Parsed from the artifact's canonical decimal-string score.
    pub score: f64,
    pub candidate_artifact_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedRuntimeOpportunitySet {
    pub artifact_id: String,
    pub market_date: String,
    pub timeframe: String,
    pub source_watchlist_hash: String,
    pub candidates: Vec<LoadedRuntimeOpportunityCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeOpportunityIntakeOutcome {
    /// [`ENV_RUNTIME_OPPORTUNITY_SET_PATH`] is absent or empty.
    NotConfigured,
    /// Path configured but file does not exist.
    Missing { configured_path: String },
    /// File found but structurally invalid, stale, wrong market date,
    /// live-approved, lineage-missing, or hash-mismatched against the
    /// bound watchlist.
    Invalid { failure_reasons: Vec<String> },
    /// Fully valid and bound to the supplied watchlist.
    Loaded {
        artifact: LoadedRuntimeOpportunitySet,
    },
}

impl RuntimeOpportunityIntakeOutcome {
    pub fn status_label(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Missing { .. } => "missing",
            Self::Invalid { .. } => "invalid",
            Self::Loaded { .. } => "loaded",
        }
    }

    pub fn artifact(&self) -> Option<&LoadedRuntimeOpportunitySet> {
        match self {
            Self::Loaded { artifact } => Some(artifact),
            _ => None,
        }
    }

    pub fn failure_reasons(&self) -> &[String] {
        match self {
            Self::Invalid { failure_reasons } => failure_reasons.as_slice(),
            _ => &[],
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical score grammar: exactly 6 fractional digits, no sign, no
// exponent, no leading zeros beyond a single "0". Mirrors
// runtime_opportunity_artifact.py::CANONICAL_SCORE_RE.
// ---------------------------------------------------------------------------

fn parse_canonical_score(raw: &str) -> Option<f64> {
    let bytes = raw.as_bytes();
    let dot = raw.find('.')?;
    let (int_part, frac_part_with_dot) = raw.split_at(dot);
    let frac_part = &frac_part_with_dot[1..];

    if frac_part.len() != 6 || !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if int_part.len() > 1 && int_part.as_bytes()[0] == b'0' {
        return None; // leading zero (e.g. "01.500000")
    }
    if bytes.iter().any(|b| !(b.is_ascii_digit() || *b == b'.')) {
        return None; // rejects sign, exponent, whitespace, etc.
    }

    raw.parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Pure evaluator
// ---------------------------------------------------------------------------

/// Evaluate a `runtime-opportunity-set-v1` artifact at `path` against the
/// already-loaded, already-approved `watchlist` and its raw JSON
/// (`watchlist_json`, needed only for the lineage-hash recomputation).
///
/// Pure aside from the single file read. Every failure reason accumulates;
/// nothing short-circuits except a missing/unreadable file or malformed JSON.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_runtime_opportunity_intake(
    path: Option<&Path>,
    watchlist: &LoadedWatchlistArtifact,
    watchlist_json: &serde_json::Value,
    now_utc: DateTime<Utc>,
    market_date: &str,
) -> RuntimeOpportunityIntakeOutcome {
    let path = match path {
        None => return RuntimeOpportunityIntakeOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => {
            return RuntimeOpportunityIntakeOutcome::NotConfigured
        }
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return RuntimeOpportunityIntakeOutcome::Missing {
                configured_path: path.display().to_string(),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return RuntimeOpportunityIntakeOutcome::Invalid {
                failure_reasons: vec![format!("invalid JSON in '{}': {e}", path.display())],
            }
        }
    };

    let mut reasons: Vec<String> = Vec::new();

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == SCHEMA_VERSION => {}
        Some(other) => reasons.push(format!(
            "runtime_opportunity_schema_invalid: unsupported schema_version '{other}'; \
             expected '{SCHEMA_VERSION}'"
        )),
        None => reasons
            .push("runtime_opportunity_schema_invalid: missing 'schema_version' field".to_string()),
    }

    match j.get("mode").and_then(|v| v.as_str()) {
        Some("paper") => {}
        _ => reasons.push("runtime_opportunity_mode_not_paper: 'mode' must be 'paper'".to_string()),
    }

    if j.get("approved_for_live").and_then(|v| v.as_bool()) != Some(false) {
        reasons.push(
            "runtime_opportunity_live_approval_forbidden: 'approved_for_live' must be false"
                .to_string(),
        );
    }

    match j.get("market_date").and_then(|v| v.as_str()) {
        Some(md) if md == market_date => {}
        _ => reasons.push(format!(
            "runtime_opportunity_wrong_market_date: expected '{market_date}'"
        )),
    }

    let expires_at_ok = j
        .get("expires_at_utc")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| now_utc <= dt.with_timezone(&Utc));
    match expires_at_ok {
        Some(true) => {}
        _ => reasons.push(
            "runtime_opportunity_stale: artifact is expired or 'expires_at_utc' is missing/invalid"
                .to_string(),
        ),
    }

    let source_watchlist_hash = j
        .get("source_watchlist_hash")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let scanner_lineage_ok = j
        .get("scanner_lineage")
        .and_then(|v| v.as_object())
        .map(|obj| obj.get("scanner_id").is_some() && obj.get("code_version").is_some())
        .unwrap_or(false);
    let has_candidate_hashes = j
        .get("source_candidate_artifact_hashes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    match &source_watchlist_hash {
        Some(_) if scanner_lineage_ok && has_candidate_hashes => {}
        _ => reasons.push(
            "runtime_opportunity_lineage_missing: source_watchlist_hash/scanner_lineage/\
             source_candidate_artifact_hashes must all be present"
                .to_string(),
        ),
    }
    if let Some(hash) = &source_watchlist_hash {
        let expected = watchlist_lineage_hash(watchlist_json);
        if *hash != expected {
            reasons.push(
                "runtime_opportunity_watchlist_hash_mismatch: source_watchlist_hash does not \
                 match the bound watchlist artifact"
                    .to_string(),
            );
        }
    }

    let artifact_id = j
        .get("artifact_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let timeframe = j
        .get("timeframe")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let candidates_json = j.get("candidates").and_then(|v| v.as_array());
    let Some(candidates_json) = candidates_json else {
        reasons
            .push("runtime_opportunity_score_missing: 'candidates' must be an array".to_string());
        return RuntimeOpportunityIntakeOutcome::Invalid {
            failure_reasons: reasons,
        };
    };

    if candidates_json.len() as u64 > MULTI_SYMBOL_HARD_CEILING {
        reasons.push(format!(
            "runtime_opportunity_symbol_count_above_ceiling: {} exceeds \
             MULTI_SYMBOL_HARD_CEILING={MULTI_SYMBOL_HARD_CEILING}",
            candidates_json.len()
        ));
    }

    let mut candidates: Vec<LoadedRuntimeOpportunityCandidate> = Vec::new();
    let mut seen_symbols: std::collections::HashSet<String> = std::collections::HashSet::new();

    for c in candidates_json {
        let symbol = c.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let strategy_id = c.get("strategy_id").and_then(|v| v.as_str()).unwrap_or("");

        if symbol.trim().is_empty() || !seen_symbols.insert(symbol.to_string()) {
            reasons.push(format!(
                "runtime_opportunity_symbol_blank_or_duplicate: '{symbol}'"
            ));
            continue;
        }

        let assigned = watchlist.strategy_assignments.get(symbol);
        if !watchlist.symbols.iter().any(|s| s == symbol)
            || assigned.map(|s| s.as_str()) != Some(strategy_id)
        {
            reasons.push(format!(
                "runtime_opportunity_symbol_strategy_mismatch: symbol '{symbol}' / strategy \
                 '{strategy_id}' does not match the bound watchlist's assignment"
            ));
            continue;
        }

        let score_str = c.get("score").and_then(|v| v.as_str());
        let score = match score_str.and_then(parse_canonical_score) {
            Some(s) if s > 0.0 => s,
            Some(_) => {
                reasons.push(format!(
                    "runtime_opportunity_score_nonpositive: symbol '{symbol}'"
                ));
                continue;
            }
            None => {
                reasons.push(format!(
                    "runtime_opportunity_score_noncanonical: symbol '{symbol}' score={score_str:?}"
                ));
                continue;
            }
        };

        if c.get("score_source").and_then(|v| v.as_str()) != Some(SCORE_SOURCE_SCANNER_TOTAL_SCORE)
        {
            reasons.push(format!(
                "runtime_opportunity_score_source_invalid: symbol '{symbol}'"
            ));
            continue;
        }
        if c.get("eligible_for_paper").and_then(|v| v.as_bool()) != Some(true) {
            reasons.push(format!(
                "runtime_opportunity_candidate_not_paper_eligible: symbol '{symbol}'"
            ));
            continue;
        }
        if c.get("eligible_for_live").and_then(|v| v.as_bool()) != Some(false) {
            reasons.push(format!(
                "runtime_opportunity_candidate_live_eligible: symbol '{symbol}'"
            ));
            continue;
        }

        let candidate_artifact_id = c
            .get("candidate_artifact_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if candidate_artifact_id.is_empty() {
            reasons.push(format!(
                "runtime_opportunity_lineage_missing: symbol '{symbol}' has no candidate_artifact_id"
            ));
            continue;
        }

        candidates.push(LoadedRuntimeOpportunityCandidate {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            score,
            candidate_artifact_id,
        });
    }

    if !reasons.is_empty() {
        return RuntimeOpportunityIntakeOutcome::Invalid {
            failure_reasons: reasons,
        };
    }

    RuntimeOpportunityIntakeOutcome::Loaded {
        artifact: LoadedRuntimeOpportunitySet {
            artifact_id,
            market_date: market_date.to_string(),
            timeframe,
            source_watchlist_hash: source_watchlist_hash.unwrap_or_default(),
            candidates,
        },
    }
}

/// [`evaluate_runtime_opportunity_intake`], reading
/// [`ENV_RUNTIME_OPPORTUNITY_SET_PATH`] from the environment.
pub fn evaluate_runtime_opportunity_intake_from_env(
    watchlist: &LoadedWatchlistArtifact,
    watchlist_json: &serde_json::Value,
    now_utc: DateTime<Utc>,
    market_date: &str,
) -> RuntimeOpportunityIntakeOutcome {
    let raw = std::env::var(ENV_RUNTIME_OPPORTUNITY_SET_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(Path::new(raw.as_str()).to_path_buf())
    };
    evaluate_runtime_opportunity_intake(
        path.as_deref(),
        watchlist,
        watchlist_json,
        now_utc,
        market_date,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    const MARKET_DATE: &str = "2026-07-26";

    fn watchlist() -> LoadedWatchlistArtifact {
        let mut sa = HashMap::new();
        sa.insert("AAPL".to_string(), "intraday_scalper".to_string());
        sa.insert("MSFT".to_string(), "intraday_scalper".to_string());
        LoadedWatchlistArtifact {
            schema_version: "watchlist-v2".to_string(),
            symbols: vec!["AAPL".to_string(), "MSFT".to_string()],
            top_symbol: Some("AAPL".to_string()),
            strategy_assignments: sa,
            max_symbols_to_trade: 2,
            max_concurrent_positions: 2,
            approved_for_autonomous_paper: true,
        }
    }

    fn watchlist_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": "watchlist-v2",
            "generated_at_utc": "2026-07-26T12:00:00+00:00",
            "symbols": ["AAPL", "MSFT"],
            "strategy_assignments": {"AAPL": "intraday_scalper", "MSFT": "intraday_scalper"},
            "max_symbols_to_trade": 2,
            "max_concurrent_positions": 2,
        })
    }

    fn valid_artifact_json() -> serde_json::Value {
        let wl_hash = watchlist_lineage_hash(&watchlist_json());
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "artifact_id": "artifact-1",
            "generated_at_utc": "2026-07-26T12:00:00+00:00",
            "expires_at_utc": "2026-07-26T12:15:00+00:00",
            "market_date": MARKET_DATE,
            "mode": "paper",
            "approved_for_autonomous_paper": true,
            "approved_for_live": false,
            "source_watchlist_hash": wl_hash,
            "source_candidate_artifact_hashes": ["cand-aapl", "cand-msft"],
            "scanner_lineage": {"scanner_id": "main_scanner", "code_version": "v1"},
            "timeframe": "5m",
            "max_symbols_to_trade": 2,
            "max_concurrent_positions": 2,
            "candidates": [
                {
                    "symbol": "AAPL",
                    "strategy_id": "intraday_scalper",
                    "score": "0.550000",
                    "score_source": SCORE_SOURCE_SCANNER_TOTAL_SCORE,
                    "candidate_artifact_id": "cand-aapl",
                    "candidate_generated_at_utc": "2026-07-26T11:55:00+00:00",
                    "eligible_for_paper": true,
                    "eligible_for_live": false
                },
                {
                    "symbol": "MSFT",
                    "strategy_id": "intraday_scalper",
                    "score": "0.100000",
                    "score_source": SCORE_SOURCE_SCANNER_TOTAL_SCORE,
                    "candidate_artifact_id": "cand-msft",
                    "candidate_generated_at_utc": "2026-07-26T11:55:00+00:00",
                    "eligible_for_paper": true,
                    "eligible_for_live": false
                }
            ]
        })
    }

    fn write_json(value: &serde_json::Value) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(serde_json::to_string(value).unwrap().as_bytes())
            .unwrap();
        f
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-26T12:05:00+00:00")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn evaluate(value: &serde_json::Value) -> RuntimeOpportunityIntakeOutcome {
        let f = write_json(value);
        evaluate_runtime_opportunity_intake(
            Some(f.path()),
            &watchlist(),
            &watchlist_json(),
            now(),
            MARKET_DATE,
        )
    }

    #[test]
    fn not_configured_when_no_path() {
        let outcome = evaluate_runtime_opportunity_intake(
            None,
            &watchlist(),
            &watchlist_json(),
            now(),
            MARKET_DATE,
        );
        assert_eq!(outcome, RuntimeOpportunityIntakeOutcome::NotConfigured);
    }

    #[test]
    fn missing_when_file_absent() {
        let outcome = evaluate_runtime_opportunity_intake(
            Some(Path::new("C:/definitely/not/a/real/path.json")),
            &watchlist(),
            &watchlist_json(),
            now(),
            MARKET_DATE,
        );
        assert_eq!(outcome.status_label(), "missing", "outcome={outcome:?}");
    }

    #[test]
    fn valid_artifact_is_loaded() {
        let outcome = evaluate(&valid_artifact_json());
        assert_eq!(outcome.status_label(), "loaded", "outcome={outcome:?}");
        let artifact = outcome.artifact().unwrap();
        assert_eq!(artifact.candidates.len(), 2);
        let aapl = artifact
            .candidates
            .iter()
            .find(|c| c.symbol == "AAPL")
            .unwrap();
        assert!((aapl.score - 0.55).abs() < 1e-9);
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let mut v = valid_artifact_json();
        v["schema_version"] = serde_json::json!("other-v1");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_schema_invalid")));
    }

    #[test]
    fn rejects_non_paper_mode() {
        let mut v = valid_artifact_json();
        v["mode"] = serde_json::json!("live");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_mode_not_paper")));
    }

    #[test]
    fn rejects_live_approval() {
        let mut v = valid_artifact_json();
        v["approved_for_live"] = serde_json::json!(true);
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_live_approval_forbidden")));
    }

    #[test]
    fn rejects_wrong_market_date() {
        let mut v = valid_artifact_json();
        v["market_date"] = serde_json::json!("2020-01-01");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_wrong_market_date")));
    }

    #[test]
    fn rejects_expired_artifact() {
        let mut v = valid_artifact_json();
        v["expires_at_utc"] = serde_json::json!("2026-07-26T12:00:00+00:00"); // before `now()`
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_stale")));
    }

    #[test]
    fn rejects_missing_lineage() {
        let mut v = valid_artifact_json();
        v.as_object_mut().unwrap().remove("source_watchlist_hash");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_lineage_missing")));
    }

    #[test]
    fn rejects_watchlist_hash_mismatch() {
        let mut v = valid_artifact_json();
        v["source_watchlist_hash"] = serde_json::json!("deadbeef");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_watchlist_hash_mismatch")));
    }

    #[test]
    fn rejects_duplicate_symbol() {
        let mut v = valid_artifact_json();
        let dup = v["candidates"][0].clone();
        v["candidates"].as_array_mut().unwrap().push(dup);
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_symbol_blank_or_duplicate")));
    }

    #[test]
    fn rejects_blank_symbol() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["symbol"] = serde_json::json!("   ");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_symbol_blank_or_duplicate")));
    }

    #[test]
    fn rejects_symbol_strategy_mismatch() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["strategy_id"] = serde_json::json!("some_other_strategy");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_symbol_strategy_mismatch")));
    }

    #[test]
    fn rejects_unknown_symbol() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["symbol"] = serde_json::json!("ZZZZ");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_symbol_strategy_mismatch")));
    }

    #[test]
    fn rejects_noncanonical_score() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["score"] = serde_json::json!("0.55");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_score_noncanonical")));
    }

    #[test]
    fn rejects_negative_score() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["score"] = serde_json::json!("-0.550000");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_score_noncanonical")));
    }

    #[test]
    fn rejects_zero_score() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["score"] = serde_json::json!("0.000000");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_score_nonpositive")));
    }

    #[test]
    fn rejects_wrong_score_source() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["score_source"] = serde_json::json!("made_up");
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_score_source_invalid")));
    }

    #[test]
    fn rejects_candidate_not_paper_eligible() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["eligible_for_paper"] = serde_json::json!(false);
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_candidate_not_paper_eligible")));
    }

    #[test]
    fn rejects_candidate_live_eligible() {
        let mut v = valid_artifact_json();
        v["candidates"][0]["eligible_for_live"] = serde_json::json!(true);
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_candidate_live_eligible")));
    }

    #[test]
    fn rejects_symbol_count_above_ceiling() {
        let mut v = valid_artifact_json();
        let mut candidates = Vec::new();
        for i in 0..(MULTI_SYMBOL_HARD_CEILING + 1) {
            candidates.push(serde_json::json!({
                "symbol": format!("SYM{i}"),
                "strategy_id": "intraday_scalper",
                "score": "0.500000",
                "score_source": SCORE_SOURCE_SCANNER_TOTAL_SCORE,
                "candidate_artifact_id": format!("cand-{i}"),
                "candidate_generated_at_utc": "2026-07-26T11:55:00+00:00",
                "eligible_for_paper": true,
                "eligible_for_live": false
            }));
        }
        v["candidates"] = serde_json::Value::Array(candidates);
        let outcome = evaluate(&v);
        assert!(outcome
            .failure_reasons()
            .iter()
            .any(|r| r.contains("runtime_opportunity_symbol_count_above_ceiling")));
    }

    #[test]
    fn watchlist_lineage_hash_is_deterministic() {
        let h1 = watchlist_lineage_hash(&watchlist_json());
        let h2 = watchlist_lineage_hash(&watchlist_json());
        assert_eq!(h1, h2);
    }

    #[test]
    fn canonical_score_parser_rejects_scientific_notation() {
        assert_eq!(parse_canonical_score("5.5e-1"), None);
    }

    #[test]
    fn canonical_score_parser_rejects_leading_zero() {
        assert_eq!(parse_canonical_score("01.500000"), None);
    }

    #[test]
    fn canonical_score_parser_accepts_valid_form() {
        assert_eq!(parse_canonical_score("0.550000"), Some(0.55));
        assert_eq!(parse_canonical_score("12.500000"), Some(12.5));
    }
}
