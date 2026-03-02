use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// A6-2: Durability policy
// ---------------------------------------------------------------------------

/// Explicit flush and rotation policy for the audit log.
///
/// # A6-2 — Log Durability Policy
///
/// Two orthogonal knobs:
///
/// | Knob                   | `strict()` | `permissive()` |
/// |------------------------|------------|----------------|
/// | `sync_on_append`       | `true`     | `false`        |
/// | `rotation_max_events`  | `0` (off)  | `0` (off)      |
///
/// ## `sync_on_append`
///
/// When `true`, `sync_all()` (equivalent to `fsync(2)`) is called after every
/// event write.  This guarantees the event reaches persistent storage before
/// `append()` returns — the event cannot be lost by an OS crash or power
/// failure after the call completes.
///
/// When `false`, durability is delegated to the OS write-back cache.  Suitable
/// for unit tests and non-critical audit streams where throughput matters more
/// than strict durability.
///
/// ## `rotation_max_events`
///
/// When non-zero, the writer starts a new log segment after this many events.
/// Segment 0 is written to the base `path`; segment N (N ≥ 1) is written to
/// `{base_path}.{N}`.  Each segment begins a fresh hash chain (independent
/// verification).  The global `seq` counter continues across segments so
/// event IDs remain unique.
///
/// `0` disables rotation (single file, no limit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurabilityPolicy {
    /// Call `sync_all()` (fsync) after every append.
    pub sync_on_append: bool,
    /// Rotate to a new segment file after this many events (0 = disabled).
    pub rotation_max_events: u64,
}

impl DurabilityPolicy {
    /// Production default: fsync per event, no rotation.
    ///
    /// Fail-closed posture: every event is durably persisted before the caller
    /// proceeds.  Use this for all live and promotion runs.
    pub fn strict() -> Self {
        Self {
            sync_on_append: true,
            rotation_max_events: 0,
        }
    }

    /// Permissive (test) default: no fsync, no rotation.
    ///
    /// Trades durability for speed.  Suitable for unit tests where the process
    /// does not crash and OS-buffered writes are sufficient.
    pub fn permissive() -> Self {
        Self {
            sync_on_append: false,
            rotation_max_events: 0,
        }
    }
}

impl Default for DurabilityPolicy {
    /// The default durability policy is `strict()` — fail-closed.
    fn default() -> Self {
        Self::strict()
    }
}

// ---------------------------------------------------------------------------
// AuditWriter
// ---------------------------------------------------------------------------

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
    /// A6-2: active durability policy.
    durability: DurabilityPolicy,
    /// A6-2: current rotation segment (0 = base path, N = `{path}.{N}`).
    segment: u64,
}

impl AuditWriter {
    /// Creates the audit writer with the default (`strict`) durability policy.
    ///
    /// Equivalent to `with_durability(path, hash_chain, DurabilityPolicy::strict())`.
    /// Use [`with_durability`] to supply an explicit policy.
    pub fn new(path: impl AsRef<Path>, hash_chain: bool) -> Result<Self> {
        Self::with_durability(path, hash_chain, DurabilityPolicy::strict())
    }

    /// Creates the audit writer with an explicit durability policy.
    ///
    /// # A6-2
    ///
    /// The caller declares the flush and rotation policy up-front.  There is
    /// no implicit default: passing `DurabilityPolicy::strict()` opts into
    /// fsync-per-event; passing `DurabilityPolicy::permissive()` explicitly
    /// opts out.
    pub fn with_durability(
        path: impl AsRef<Path>,
        hash_chain: bool,
        durability: DurabilityPolicy,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }

        Ok(Self {
            path,
            hash_chain,
            last_hash: None,
            seq: 0,
            durability,
            segment: 0,
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

    /// A6-2: Set the rotation segment counter (for restart with rotation enabled).
    pub fn set_segment(&mut self, segment: u64) {
        self.segment = segment;
    }

    /// A6-2: Current rotation segment index (0 = base path).
    pub fn segment(&self) -> u64 {
        self.segment
    }

    /// A6-2: Absolute path of the current segment file being written.
    ///
    /// Segment 0 → `{path}` (the base path).
    /// Segment N → `{path}.{N}` for N ≥ 1.
    pub fn current_segment_path(&self) -> PathBuf {
        if self.segment == 0 {
            self.path.clone()
        } else {
            let fname = self
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let mut p = self.path.clone();
            p.set_file_name(format!("{}.{}", fname, self.segment));
            p
        }
    }

    /// Append one event.
    pub fn append(
        &mut self,
        run_id: Uuid,
        topic: &str,
        event_type: &str,
        payload: Value,
    ) -> Result<AuditEvent> {
        // A6-2: rotation check — before writing, advance segment if threshold reached.
        if self.durability.rotation_max_events > 0
            && self.seq > 0
            && self.seq.is_multiple_of(self.durability.rotation_max_events)
        {
            self.segment += 1;
            // Each segment starts a fresh hash chain so it is independently verifiable.
            // The global seq continues, keeping event_ids unique across segments.
            if self.hash_chain {
                self.last_hash = None;
            }
        }

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
        // A6-2: write to the current segment path; flush according to policy.
        let seg_path = self.current_segment_path();
        append_line(&seg_path, &line, self.durability.sync_on_append)?;

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

// ---------------------------------------------------------------------------
// Event-ID derivation (D1-2)
// ---------------------------------------------------------------------------

/// Derive a deterministic audit event ID from chain state, payload, and sequence.
///
/// **No RNG.** Uses `Uuid::new_v5` (SHA-1 over the DNS namespace).
///
/// Inputs (D1-2 contract: `prev_hash + payload_hash + seq`):
///   `prev_hash` — `hash_self` of the previous event, or `None` for the first event.
///   `payload`   — the event payload (canonicalized for key-order stability).
///   `seq`       — monotonically increasing counter from `AuditWriter` (prevents
///                 collisions when `prev_hash` and `payload` are identical).
///
/// `ts_utc` is intentionally excluded: it is wall-clock ops metadata and its
/// non-determinism is addressed separately. Including it would re-introduce
/// non-determinism into `event_id` even after the TimeSource abstraction (D1-3)
/// is in place.
fn derive_event_id(prev_hash: Option<&str>, payload: &Value, seq: u64) -> Result<Uuid> {
    let payload_canonical = canonical_json_line(payload)?;
    let prev = prev_hash.unwrap_or("");
    let data = format!("mqk-audit.event.v1|{}|{}|{}", prev, payload_canonical, seq);
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_DNS, data.as_bytes()))
}

// ---------------------------------------------------------------------------
// File I/O
// ---------------------------------------------------------------------------

/// Write a single line to file (with trailing newline).
///
/// # A6-2 — flush policy
///
/// When `sync` is `true`, `sync_all()` (fsync) is called after the write.
/// This guarantees the kernel has flushed the data to persistent storage
/// before the function returns.
///
/// When `sync` is `false`, durability is delegated to the OS write-back cache.
fn append_line(path: &Path, line: &str, sync: bool) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open audit log {:?}", path))?;
    f.write_all(line.as_bytes())
        .context("write audit line failed")?;
    f.write_all(b"\n").context("write newline failed")?;
    if sync {
        f.sync_all().context("sync_all (fsync) failed")?;
    }
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
