use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Append-only audit writer. Writes JSON Lines (one event per line).
/// Optional hash chain: each event can include hash_prev + hash_self.
pub struct AuditWriter {
    path: PathBuf,
    hash_chain: bool,
    last_hash: Option<String>,
    /// Monotonically increasing sequence counter for `event_id` derivation (D1-2).
    /// Starts at 0 and increments on every `append` call.
    /// When resuming an existing log (e.g. after daemon restart), restore with
    /// `set_seq(events_already_written)` alongside `set_last_hash`.
    seq: u64,
}

impl AuditWriter {
    /// Creates the audit writer and ensures parent dirs exist.
    pub fn new(path: impl AsRef<Path>, hash_chain: bool) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }

        Ok(Self {
            path,
            hash_chain,
            last_hash: None,
            seq: 0,
        })
    }

    /// Set last hash explicitly (e.g., after reading last line on restart).
    pub fn set_last_hash(&mut self, last_hash: Option<String>) {
        self.last_hash = last_hash;
    }

    pub fn last_hash(&self) -> Option<String> {
        self.last_hash.clone()
    }

    /// Set the sequence counter when resuming an existing log after restart.
    /// Pass the number of events already written (the next event's seq = this value).
    /// Must be called in conjunction with `set_last_hash` for correct restart semantics.
    pub fn set_seq(&mut self, seq: u64) {
        self.seq = seq;
    }

    /// Current sequence counter (equals the number of events appended so far).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Append one event.
    pub fn append(
        &mut self,
        run_id: Uuid,
        topic: &str,
        event_type: &str,
        payload: Value,
    ) -> Result<AuditEvent> {
        let ts_utc = Utc::now();
        // D1-2: event_id derived deterministically from chain state + payload + seq.
        // No RNG. See `derive_event_id` for derivation contract.
        let event_id = derive_event_id(self.last_hash.as_deref(), &payload, self.seq)?;
        self.seq += 1;

        let mut ev = AuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: topic.to_string(),
            event_type: event_type.to_string(),
            payload,
            hash_prev: None,
            hash_self: None,
        };

        if self.hash_chain {
            let prev = self.last_hash.clone();
            ev.hash_prev = prev;

            let self_hash = compute_event_hash(&ev)?;
            ev.hash_self = Some(self_hash.clone());
            self.last_hash = Some(self_hash);
        }

        let line = canonical_json_line(&ev)?;
        append_line(&self.path, &line)?;

        Ok(ev)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: Uuid,
    pub run_id: Uuid,
    pub ts_utc: DateTime<Utc>,
    pub topic: String,
    pub event_type: String,
    pub payload: Value,
    pub hash_prev: Option<String>,
    pub hash_self: Option<String>,
}

/// Write a single line to file (with trailing newline).
fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open audit log {:?}", path))?;
    f.write_all(line.as_bytes())
        .context("write audit line failed")?;
    f.write_all(b"\n").context("write newline failed")?;
    Ok(())
}

/// Canonicalize by sorting keys recursively and emitting compact JSON.
/// One event == one JSON line.
fn canonical_json_line<T: Serialize>(v: &T) -> Result<String> {
    let raw = serde_json::to_value(v).context("serialize audit event failed")?;
    let sorted = sort_keys(&raw);
    serde_json::to_string(&sorted).context("json stringify failed")
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            let mut new = serde_json::Map::new();
            for k in keys {
                new.insert(k.clone(), sort_keys(&map[&k]));
            }
            Value::Object(new)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_keys).collect()),
        _ => v.clone(),
    }
}

/// Hash chain is computed from canonical JSON of event WITHOUT hash_self (to avoid self-reference).
pub fn compute_event_hash(ev: &AuditEvent) -> Result<String> {
    let mut clone = ev.clone();
    clone.hash_self = None;

    let canonical = canonical_json_line(&clone)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Verify the hash chain integrity of an audit log file.
///
/// Returns Ok(VerifyResult) describing whether the chain is intact or where it breaks.
///
/// PATCH 15c: required by docs/specs/run_artifacts_and_reproducibility.md.
pub fn verify_hash_chain(path: impl AsRef<Path>) -> Result<VerifyResult> {
    let content = fs::read_to_string(path.as_ref())
        .with_context(|| format!("read audit log {:?}", path.as_ref()))?;
    verify_hash_chain_str(&content)
}

/// Verify the hash chain integrity of an audit log string (JSONL content).
///
/// Same logic as [`verify_hash_chain`] but operates on an in-memory `&str`.
/// Useful for testing and for the Patch B6 artifact gate, which validates
/// audit logs without requiring a file path.
pub fn verify_hash_chain_str(content: &str) -> Result<VerifyResult> {
    let mut prev_hash: Option<String> = None;
    let mut line_count = 0usize;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let ev: AuditEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("parse audit event at line {}", i + 1))?;

        line_count += 1;

        // 1. Verify hash_prev matches the previous event's hash_self
        if ev.hash_prev != prev_hash {
            return Ok(VerifyResult::Broken {
                line: i + 1,
                reason: format!(
                    "hash_prev mismatch: expected {:?}, got {:?}",
                    prev_hash, ev.hash_prev
                ),
            });
        }

        // 2. Verify hash_self is correct for this event's content
        if let Some(ref claimed_hash) = ev.hash_self {
            let recomputed = compute_event_hash(&ev)?;
            if *claimed_hash != recomputed {
                return Ok(VerifyResult::Broken {
                    line: i + 1,
                    reason: format!(
                        "hash_self mismatch: claimed {}, recomputed {}",
                        claimed_hash, recomputed
                    ),
                });
            }
        }

        prev_hash = ev.hash_self.clone();
    }

    Ok(VerifyResult::Valid { lines: line_count })
}

/// Result of hash chain verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// The entire chain is valid.
    Valid { lines: usize },
    /// The chain is broken at the given line.
    Broken { line: usize, reason: String },
}
