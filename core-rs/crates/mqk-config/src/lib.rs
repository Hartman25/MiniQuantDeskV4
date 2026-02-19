use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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

/// PATCH 26 — Config Consumption Map + Unused-Key Guard (Safety Lint)
///
/// We intentionally do NOT add new config keys.
/// Consumers (CLI/orchestrator/etc.) choose whether unused keys are warnings or errors
/// by calling `report_unused_keys(mode, &config_json, UnusedKeyPolicy::Warn|Fail)`.
///
/// “Consumed pointers” are JSON Pointer prefixes. If a leaf pointer is under any consumed
/// prefix, that leaf is considered consumed. Any leaf not covered is "unused".
///
/// Examples:
/// - consumed prefix "/runtime" consumes "/runtime/log_level" and "/runtime/data/source"
/// - consumed prefix "/risk/limits" consumes "/risk/limits/max_notional"
///
/// NOTE: The registry should evolve as modes mature. Start conservative: only mark keys
/// you KNOW are read in that mode.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    Backtest,
    Paper,
    Live,
}

impl ConfigMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigMode::Backtest => "BACKTEST",
            ConfigMode::Paper => "PAPER",
            ConfigMode::Live => "LIVE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusedKeyPolicy {
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnusedKeyReport {
    pub mode: String,
    /// Consumed JSON-pointer prefixes used for this analysis (sorted, unique)
    pub consumed_prefixes: Vec<String>,
    /// Minimal set of unused leaf pointers (sorted)
    pub unused_leaf_pointers: Vec<String>,
}

impl UnusedKeyReport {
    pub fn is_clean(&self) -> bool {
        self.unused_leaf_pointers.is_empty()
    }
}

/// Registry of consumed JSON-pointer prefixes per mode.
///
/// IMPORTANT:
/// - Do not add new config keys.
/// - Do not "wish-consume" keys. Only list what is actually read for that mode.
/// - Prefer broader prefixes only when you truly consume a whole subtree.
pub fn consumed_pointers_for_mode(mode: ConfigMode) -> &'static [&'static str] {
    match mode {
        // Backtest typically uses data feed + execution + risk + promotion gates, etc.
        // Keep conservative; expand as you confirm reads.
        ConfigMode::Backtest => &[
            "/runtime",
            "/data",
            "/execution",
            "/risk",
            "/strategy",
            "/backtest",
            "/promotion",
            "/integrity",
            "/reconcile",
            "/portfolio",
            "/audit",
            "/artifacts",
        ],
        // Paper is closer to live but without real money; still uses broker/runtime/risk/etc.
        ConfigMode::Paper => &[
            "/runtime",
            "/data",
            "/execution",
            "/risk",
            "/strategy",
            "/paper",
            "/broker",
            "/integrity",
            "/reconcile",
            "/portfolio",
            "/audit",
            "/artifacts",
        ],
        // Live should be strictest; includes broker/live/runtime/etc.
        ConfigMode::Live => &[
            "/runtime",
            "/data",
            "/execution",
            "/risk",
            "/strategy",
            "/live",
            "/broker",
            "/integrity",
            "/reconcile",
            "/portfolio",
            "/audit",
            "/artifacts",
        ],
    }
}

/// Produce an unused-key report for a given mode.
/// If `policy == Fail`, returns an error when unused keys exist.
/// If `policy == Warn`, always returns Ok(report).
pub fn report_unused_keys(
    mode: ConfigMode,
    config_json: &Value,
    policy: UnusedKeyPolicy,
) -> Result<UnusedKeyReport> {
    // Normalize prefixes: unique + sorted
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    for p in consumed_pointers_for_mode(mode) {
        consumed.insert(normalize_pointer(p));
    }
    let consumed_prefixes: Vec<String> = consumed.iter().cloned().collect();

    // Collect all leaf pointers in config
    let mut leaves: Vec<String> = Vec::new();
    collect_leaf_pointers(config_json, "", &mut leaves);

    // Determine unused leaves: not under any consumed prefix
    let mut unused: Vec<String> = Vec::new();
    'leaf: for lp in leaves {
        for cp in &consumed_prefixes {
            if is_prefix_pointer(cp, &lp) {
                continue 'leaf;
            }
        }
        unused.push(lp);
    }

    unused.sort();
    unused.dedup();

    let report = UnusedKeyReport {
        mode: mode.as_str().to_string(),
        consumed_prefixes,
        unused_leaf_pointers: unused,
    };

    if policy == UnusedKeyPolicy::Fail && !report.is_clean() {
        // Keep message deterministic and copy/paste friendly.
        bail!(
            "CONFIG_UNUSED_KEYS (mode={}): {} unused config leaf key(s) detected. \
            Remove them or update the consumed registry. First few: {}",
            report.mode,
            report.unused_leaf_pointers.len(),
            preview_list(&report.unused_leaf_pointers, 12)
        );
    }

    Ok(report)
}

/// Normalize JSON pointer:
/// - Empty => "" (root)
/// - Ensure starts with '/'
fn normalize_pointer(p: &str) -> String {
    let t = p.trim();
    if t.is_empty() {
        return "".to_string();
    }
    if t.starts_with('/') {
        t.to_string()
    } else {
        format!("/{t}")
    }
}

/// True if `prefix` is a JSON-pointer prefix of `full`.
/// Root prefix "" matches everything.
fn is_prefix_pointer(prefix: &str, full: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if prefix == full {
        return true;
    }
    // prefix must match segment boundary: "/a/b" matches "/a/b/c" but not "/a/bc"
    if let Some(rest) = full.strip_prefix(prefix) {
        return rest.starts_with('/');
    }
    false
}

/// Collect JSON pointer strings for all leaf values in the JSON.
/// Leaf = anything that is not an object or array.
/// We include explicit array indices as segments (e.g. "/risk/limits/tiers/0/max").
fn collect_leaf_pointers(v: &Value, cur: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let next = if cur.is_empty() {
                    format!("/{}", escape_pointer_token(k))
                } else {
                    format!("{}/{}", cur, escape_pointer_token(k))
                };
                collect_leaf_pointers(child, &next, out);
            }
        }
        Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                let next = if cur.is_empty() {
                    format!("/{}", i)
                } else {
                    format!("{}/{}", cur, i)
                };
                collect_leaf_pointers(child, &next, out);
            }
        }
        _ => {
            // Leaf scalar
            if cur.is_empty() {
                // root leaf
                out.push("".to_string());
            } else {
                out.push(cur.to_string());
            }
        }
    }
}

/// Escape a JSON pointer token per RFC6901:
/// "~" => "~0", "/" => "~1"
fn escape_pointer_token(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn preview_list(xs: &[String], max: usize) -> String {
    let take = xs.iter().take(max).cloned().collect::<Vec<_>>();
    if xs.len() <= max {
        format!("{:?}", take)
    } else {
        format!("{:?} ... (+{})", take, xs.len().saturating_sub(max))
    }
}

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

    // Canonicalize (stable key order) by sorting object keys recursively and emitting compact JSON.
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
