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
/// This is intentionally conservative and should only include pointers actually
/// read by code in that mode.
pub fn consumed_pointers_for_mode(mode: ConfigMode) -> &'static [&'static str] {
    match mode {
        // IMPORTANT:
        // This registry must reflect what the code ACTUALLY reads today.
        // Do not "wish-consume" broad sections.
        //
        // Observed config reads (Feb 2026):
        // - mqk-isolation::EngineIsolation::from_config_json
        //     /engine/engine_id
        //     /broker/keys_env/api_key
        //     /broker/keys_env/api_secret
        //     /risk/max_gross_exposure
        // - mqk-cli::enforce_manual_confirmation_if_required (LIVE only)
        //     /arming/require_manual_confirmation
        //     /arming/confirmation_format
        //     /broker/account_last4
        //     /risk/daily_loss_limit
        // - mqk-db::arm_preflight (LIVE only; some reads conditional)
        //     /arming/require_clean_reconcile           (read always, enforced in LIVE only)
        //     /risk/daily_loss_limit                    (LIVE only)
        //     /risk/max_drawdown                        (LIVE only; optional)
        //     /arming/require_killswitch_policies       (LIVE only)
        //     /data/stale_policy                        (LIVE only; when require_killswitch_policies=true)
        //     /data/feed_disagreement_policy            (LIVE only; when require_killswitch_policies=true)
        //     /risk/reject_storm/max_rejects            (LIVE only; when require_killswitch_policies=true)
        //
        // NOTE: When PAPER/BACKTEST mature (runtime loop, broker wiring, etc.),
        // expand their registries to match new reads.
        ConfigMode::Backtest => &[
            "/engine/engine_id",
            "/broker/keys_env/api_key",
            "/broker/keys_env/api_secret",
            "/risk/max_gross_exposure",
        ],

        ConfigMode::Paper => &[
            "/engine/engine_id",
            "/broker/keys_env/api_key",
            "/broker/keys_env/api_secret",
            "/risk/max_gross_exposure",
        ],

        ConfigMode::Live => &[
            "/engine/engine_id",
            "/broker/keys_env/api_key",
            "/broker/keys_env/api_secret",
            "/risk/max_gross_exposure",
            // CLI manual-confirmation gate (mqk-cli)
            "/arming/require_manual_confirmation",
            "/arming/confirmation_format",
            "/broker/account_last4",
            "/risk/daily_loss_limit",
            // DB arm_preflight() (mqk-db)
            "/arming/require_clean_reconcile",
            "/risk/max_drawdown",
            "/arming/require_killswitch_policies",
            "/data/stale_policy",
            "/data/feed_disagreement_policy",
            "/risk/reject_storm/max_rejects",
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
/// - must begin with "/"
/// - no trailing "/" unless it's just "/"
fn normalize_pointer(p: &str) -> String {
    let mut s = p.trim().to_string();
    if s.is_empty() {
        return "/".to_string();
    }
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    s
}

/// Return true if `prefix` is a JSON-pointer prefix of `leaf`.
///
/// Rules:
/// - prefix "/" consumes everything
/// - exact match consumes
/// - "/a/b" consumes "/a/b/c" but NOT "/a/bc"
fn is_prefix_pointer(prefix: &str, leaf: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    if leaf == prefix {
        return true;
    }
    if leaf.starts_with(prefix) {
        // Ensure boundary at next char is "/"
        return leaf
            .get(prefix.len()..prefix.len() + 1)
            .map(|c| c == "/")
            .unwrap_or(false);
    }
    false
}

fn collect_leaf_pointers(v: &Value, prefix: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, vv) in map.iter() {
                let next = format!("{}/{}", prefix, escape_pointer_token(k));
                collect_leaf_pointers(vv, &next, out);
            }
        }
        Value::Array(arr) => {
            for (i, vv) in arr.iter().enumerate() {
                let next = format!("{}/{}", prefix, i);
                collect_leaf_pointers(vv, &next, out);
            }
        }
        _ => {
            // Leaf
            let p = if prefix.is_empty() {
                "/".to_string()
            } else {
                prefix.to_string()
            };
            out.push(p);
        }
    }
}

fn escape_pointer_token(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn preview_list(items: &[String], n: usize) -> String {
    let take = items.iter().take(n).cloned().collect::<Vec<_>>();
    format!("{:?}", take)
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config_hash: String,
    pub canonical_json: String,
    pub config_json: Value,
}

pub fn load_layered_yaml(paths: &[&str]) -> Result<LoadedConfig> {
    let mut docs: Vec<String> = Vec::new();
    for p in paths {
        let raw =
            fs::read_to_string(p).with_context(|| format!("failed to read yaml path: {p}"))?;
        docs.push(raw);
    }

    // Convert Vec<String> -> Vec<&str> to match load_layered_yaml_from_strings signature.
    let doc_refs: Vec<&str> = docs.iter().map(|s| s.as_str()).collect();
    load_layered_yaml_from_strings(&doc_refs)
}

pub fn load_layered_yaml_from_strings(yaml_docs: &[&str]) -> Result<LoadedConfig> {
    // Merge YAML docs in order: earlier docs are base, later docs override.
    let mut merged = serde_json::json!({});
    for raw in yaml_docs {
        let v_yaml: serde_yaml::Value = serde_yaml::from_str(raw).context("invalid yaml")?;
        let v_json = serde_json::to_value(v_yaml).context("yaml->json conversion failed")?;
        merged = deep_merge(merged, v_json);
    }

    // Enforce "no secrets as literal values" policy.
    enforce_no_secret_literals(&merged)?;

    let canonical_json = canonicalize_json(&merged)?;
    let config_hash = sha256_hex(canonical_json.as_bytes());
    Ok(LoadedConfig {
        config_hash,
        canonical_json,
        config_json: merged,
    })
}

fn deep_merge(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Object(mut a_map), Value::Object(b_map)) => {
            for (k, b_val) in b_map {
                let a_val = a_map.remove(&k).unwrap_or(Value::Null);
                a_map.insert(k, deep_merge(a_val, b_val));
            }
            Value::Object(a_map)
        }
        (_, b_other) => b_other,
    }
}

fn canonicalize_json(v: &Value) -> Result<String> {
    // Deterministic ordering is guaranteed by serde_json for map keys when serializing
    // if we use a BTreeMap-like representation; serde_json::Value uses Map which preserves
    // insertion order, but our merge logic is deterministic given deterministic YAML input ordering.
    // Still, we serialize with pretty formatting disabled and stable float rendering.
    let s = serde_json::to_string(v).context("canonical json serialize failed")?;
    Ok(s)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    hex::encode(out)
}

fn enforce_no_secret_literals(v: &Value) -> Result<()> {
    // Walk leaf strings; reject if they look like a secret.
    let mut leaves = Vec::new();
    collect_leaf_pointers(v, "", &mut leaves);

    for ptr in leaves {
        if let Some(val) = v.pointer(&ptr) {
            if let Some(s) = val.as_str() {
                if looks_like_secret(s) {
                    bail!("CONFIG_SECRET_DETECTED leaf={} value=REDACTED", ptr);
                }
            }
        }
    }
    Ok(())
}

fn looks_like_secret(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 8 {
        return false;
    }
    SECRET_PREFIXES.iter().any(|p| t.starts_with(p))
}
