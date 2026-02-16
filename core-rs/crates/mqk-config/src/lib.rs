use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;

/// Load + merge YAML files in order, then canonicalize to JSON and hash.
/// Later files override earlier files via deep-merge.
pub fn load_layered_yaml(paths: &[&str]) -> Result<LoadedConfig> {
    let mut merged = Value::Object(Default::default());

    for p in paths {
        let s = fs::read_to_string(p).with_context(|| format!("read config: {p}"))?;
        let yaml_val: serde_yaml::Value =
            serde_yaml::from_str(&s).with_context(|| format!("parse yaml: {p}"))?;
        let json_val = serde_json::to_value(yaml_val).context("yaml->json conversion failed")?;
        deep_merge(&mut merged, json_val);
    }

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
