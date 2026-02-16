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
        })
    }

    /// Set last hash explicitly (e.g., after reading last line on restart).
    pub fn set_last_hash(&mut self, last_hash: Option<String>) {
        self.last_hash = last_hash;
    }

    pub fn last_hash(&self) -> Option<String> {
        self.last_hash.clone()
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
        let event_id = Uuid::new_v4();

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
    f.write_all(line.as_bytes()).context("write audit line failed")?;
    f.write_all(b"\n").context("write newline failed")?;
    Ok(())
}

/// Canonicalize by sorting keys recursively and emitting compact JSON.
/// One event == one JSON line.
fn canonical_json_line<T: Serialize>(v: &T) -> Result<String> {
    let raw = serde_json::to_value(v).context("serialize audit event failed")?;
    let sorted = sort_keys(&raw);
    Ok(serde_json::to_string(&sorted).context("json stringify failed")?)
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
fn compute_event_hash(ev: &AuditEvent) -> Result<String> {
    let mut clone = ev.clone();
    clone.hash_self = None;

    let canonical = canonical_json_line(&clone)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}
