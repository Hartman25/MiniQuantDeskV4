use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;

/// Known secret-like prefixes / patterns. If any leaf string value in the
/// effective config starts with one of these, we abort with CONFIG_SECRET_DETECTED.
/// This implements docs/specs/config_layering_and_hashing.md section 5.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",        // Stripe / OpenAI style
    "sk_live",    // Stripe live
    "sk_test",    // Stripe test
    "AKIA",       // AWS access key ID
    "-----BEGIN", // PEM private keys
    "ghp_",       // GitHub PAT
    "gho_",       // GitHub OAuth
    "glpat-",     // GitLab PAT
    "xoxb-",      // Slack bot token
    "xoxp-",      // Slack user token
];

/// Load + merge YAML files in order, then canonicalize to JSON and hash.
/// Later files override earlier files via deep-merge.
///
/// After merging, scans all leaf string values for secret-like patterns.
/// If a secret is detected, aborts with CONFIG_SECRET_DETECTED (per spec).
pub fn load_layered_yaml(paths: &[&str]) -> Result<LoadedConfig> {
    let mut merged = Value::Object(Default::default());

    for p in paths {
        let s = fs::read_to_string(p).with_context(|| format!("read config: {p}"))?;
        let yaml_val: serde_yaml::Value =
            serde_yaml::from_str(&s).with_context(|| format!("parse yaml: {p}"))?;
        let json_val = serde_json::to_value(yaml_val).context("yaml->json conversion failed")?;
        deep_merge(&mut merged, json_val);
    }

    // PATCH 15a / spec section 5: scan for secrets before hashing or storing.
    scan_for_secrets(&merged)?;

    // Canonicalize (stable key order) by round-tripping through serde_json::to_string,
    // which orders keys deterministically for maps (BTreeMap) only if we ensure sorting.
    // So we implement a manual canonicalization step that sorts object keys.
    let canonical = canonicalize_json(&merged);

    // Hash canonical bytes
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hash = hex::encode(hasher.finalize());

    Ok(LoadedConfig {
        config_json: serde_json::from_str(&canonical).context("canonical json parse failed")?,
        canonical_json: canonical,
        config_hash: hash,
    })
}

/// Also expose a from-string loader for testing without filesystem.
pub fn load_layered_yaml_from_strings(yamls: &[&str]) -> Result<LoadedConfig> {
    let mut merged = Value::Object(Default::default());

    for (i, s) in yamls.iter().enumerate() {
        let yaml_val: serde_yaml::Value =
            serde_yaml::from_str(s).with_context(|| format!("parse yaml string #{i}"))?;
        let json_val = serde_json::to_value(yaml_val).context("yaml->json conversion failed")?;
        deep_merge(&mut merged, json_val);
    }

    scan_for_secrets(&merged)?;

    let canonical = canonicalize_json(&merged);

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hash = hex::encode(hasher.finalize());

    Ok(LoadedConfig {
        config_json: serde_json::from_str(&canonical).context("canonical json parse failed")?,
        canonical_json: canonical,
        config_hash: hash,
    })
}

/// Recursively scan all leaf string values for secret-like patterns.
/// Aborts with CONFIG_SECRET_DETECTED if any match is found.
fn scan_for_secrets(v: &Value) -> Result<()> {
    scan_for_secrets_inner(v, "")
}

fn scan_for_secrets_inner(v: &Value, path: &str) -> Result<()> {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                scan_for_secrets_inner(child, &child_path)?;
            }
        }
        Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                scan_for_secrets_inner(child, &format!("{path}[{i}]"))?;
            }
        }
        Value::String(s) => {
            if is_secret_like(s) {
                bail!(
                    "CONFIG_SECRET_DETECTED at '{path}': value starts with a known secret prefix (redacted). \
                     Secrets must never appear as literal values in config. \
                     Use env var NAMES instead (e.g., api_key_env: \"MY_API_KEY\")."
                );
            }
        }
        _ => {}
    }
    Ok(())
}

/// Check if a string value looks like an actual secret (not an env var name).
fn is_secret_like(s: &str) -> bool {
    let trimmed = s.trim();
    for prefix in SECRET_PREFIXES {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config_json: Value,
    pub canonical_json: String,
    pub config_hash: String,
}

/// Deep-merge: objects merge recursively; arrays replaced; scalars overwritten.
fn deep_merge(dst: &mut Value, src: Value) {
    match (dst, src) {
        (Value::Object(dst_map), Value::Object(src_map)) => {
            for (k, v) in src_map {
                match dst_map.get_mut(&k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        dst_map.insert(k, v);
                    }
                }
            }
        }
        (dst_slot, src_val) => {
            *dst_slot = src_val;
        }
    }
}

/// Canonicalize JSON by sorting all object keys recursively and emitting compact JSON.
fn canonicalize_json(v: &Value) -> String {
    let sorted = sort_keys(v);
    serde_json::to_string(&sorted).expect("json serialization must not fail")
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
