# Short-Side Paper-Enablement Audit (SHORT-SIDE-PAPER-ENABLEMENT-AUDIT-01)

Audit-only reference: what must change before MiniQuantDeskV4 may allow a **real
paper-only short entry**. This document changes **no behavior**. It does not
enable short entries, does not modify B5, does not weaken any risk gate, and
does not touch broker submit or live-routing code.

Per `.claude/rules/audit_repo_truth_rules.md` this file is deliberately written
against **stable file/function names and gate semantics**, not commit hashes or
open/closed status tables, so it stays valid as patches land. Verify every claim
against the current HEAD before acting on it.

---

## 1. The single structural blocker today: B5

Short opens are blocked in exactly one place on the live decision path:

- `mqk-daemon/src/decision.rs` → `bar_result_to_decisions()`. After classifying
  each `(current_qty, delta)` via `capital_policy::classify_order_intent`, the
  intents `OrderIntent::ShortOpen` and `OrderIntent::SellBeyondLongToShort`
  `return None` — the target never becomes an `InternalStrategyDecision`, so it
  never reaches `submit_internal_strategy_decision`, the outbox, the
  orchestrator, or any broker. This is **mode-independent**: it fires the same
  in paper and live.

Because B5 drops short intents *before a decision exists*, every downstream gate
(`submit_internal_strategy_decision` Gates 0–7, the orchestrator, broker submit)
**never sees a short-open**. That is why broker submit code needs no change to
keep shorts blocked, and why it needs no change to *initially* allow a paper
short either — it is already side-agnostic (`side: "buy" | "sell"`).

## 2. The short-entry policy module is proven but UNWIRED

`mqk-daemon/src/capital_policy/short_entry_policy.rs` provides a complete,
fail-closed policy evaluator (`evaluate_short_entry_policy`,
`evaluate_short_entry_policy_with_preflight`, `ShortEntryConfig`,
`parse_short_entry_config`, intent classifiers, and the
`ShortabilityPreflightResult` seam). It is fully unit-proven.

It is **not called on the order path.** The only production caller of
`evaluate_short_entry_policy` is `state/dry_run_strategy.rs`
(`would_default_short_entry_policy_block`), which is a read-only diagnostic and
hardcodes `ShortEntryConfig::fail_closed()`. `parse_short_entry_config` (the real
policy-file loader) has **no production caller at all** — there is no
`parse_short_entry_config_from_env`. So today the policy module informs
diagnostics only; B5 is the enforcement.

Enabling paper shorts therefore is **not** "flip a config flag." It requires
*wiring* the policy as a gate, gated strictly behind B5, behind a real
shortability check, and behind paper-mode.

## 3. Shortability preflight is a parser + seam only — no live lookup

- `mqk-broker-alpaca/src/types.rs`: `AlpacaAssetRaw` + `parse_alpaca_asset_json`
  exist (pure JSON deserialization of `GET /v2/assets/{symbol}`).
- There is **no HTTP method** that performs that lookup. The Alpaca adapter
  (`mqk-broker-alpaca/src/lib.rs`) implements `submit_order`, `cancel_order`,
  `replace_order`, `fetch_events`, `fetch_broker_snapshot` — and **no**
  `get_asset` / shortability fetch.
- `ShortabilitySource::AlpacaAsset` is constructed only in a test.

So shortability is provably unable to confirm "shortable" from a real broker
call today; the policy's `require_shortable_check=true` default would reject with
`shortable_check_required` / `shortable_check_unavailable` if it were wired.

## 4. Live shorting is hard-stopped in two independent ways

1. **B5** drops all short opens regardless of mode (see §1).
2. If/when the policy is wired, `evaluate_short_entry_policy` /
   `_with_preflight` **unconditionally** return `ShortOpenBlocked
   { reason_code: "live_short_entries_blocked" }` when `is_paper_mode == false`,
   *before* consulting `allow_short_entries`, `live_short_entries_enabled`, or
   any shortability result. The live block cannot be reached around.

`live_routing_enabled` (`routes/helpers.rs::environment_and_live_routing_truth`)
is a status-surface truth derived from run mode; it is **not** the short gate. The
short hard-stops are B5 + the policy's paper-mode gate.

> Invariant for all future short patches: **live shorting stays permanently
> blocked.** No short patch may consult `live_short_entries_enabled` for live
> mode, and none may move the B5/policy block behind the live-mode check.

## 5. Already-proven supporting stack (no change needed to enable paper short)

- **Strategy emission**: `mqk-strategy/src/engines/intraday_scalper.rs` —
  `intraday_scalper` is long-only (`allow_short_signals=false`);
  `intraday_short_scalper` (`new_short`, `SHORT_NAME`) emits negative targets and
  suppresses longs. Both are registered by `engines::register_builtin_strategies`
  (and thus by `mqk-runtime::native_strategy::build_daemon_plugin_registry`).
- **Dry-run isolation**: `state/dry_run_strategy.rs` takes no
  `AppState`/`PgPool`/broker handle — secondary strategies are *structurally*
  unable to submit. `submitted` is always `false`.
- **Paper fills**: `mqk-broker-paper/src/fill_engine.rs` — short-sell and
  buy-to-cover fills carry positive `delta_qty` and correct `Side`.
- **Portfolio**: `mqk-portfolio/src/accounting.rs` — FIFO opens short lots on
  sell-beyond-long and covers shorts first on buy.
- **Reconcile**: `mqk-reconcile/src/{types.rs,snapshot_adapter.rs}` — positions
  are `qty_signed` (negative = short); `normalize_side` maps `"short" → Sell`.

## 6. Gate-by-gate enablement map

| Gate / Layer | Current behavior | Required paper-only behavior | Change? | File / function |
|---|---|---|---|---|
| Strategy emission | `intraday_short_scalper` emits negative target; long-only default unchanged | A **primary** paper short strategy may emit negative target | no (engine exists) | `engines/intraday_scalper.rs` (`new_short`) |
| Strategy selection | `MQK_STRATEGY_IDS` → `NativeStrategyBootstrap`; short variant registered | Short strategy selectable as primary | no (already registrable) | `state/lifecycle.rs`, `native_strategy.rs` |
| **B5** | drops `ShortOpen` / `SellBeyondLongToShort` for all modes | allow a short open **only if** the paper short-entry policy authorizes it | **yes** | `decision.rs::bar_result_to_decisions` |
| Short policy | proven but **unwired**; `fail_closed()` only, diagnostic only | wired as a real gate; allow only paper + flags + caps + shortability | **yes (wire it)** | `capital_policy/short_entry_policy.rs`, `parse_short_entry_config` |
| Shortability | pure parser + seam; **no HTTP** | real Alpaca `GET /v2/assets/{symbol}` **or** explicit paper-test override | **yes** | `mqk-broker-alpaca` (new method) + `ShortabilityPreflightResult` |
| Capital budget (Gate 1e) | unset env → `PolicyNotConfigured` → passes through | if a policy file is introduced, short strategy needs an explicit `budget_authorized=true` entry | maybe | `capital_policy/mod.rs::evaluate_strategy_budget` |
| Conflict policy | only one primary strategy dispatches; long+short concurrent not wired | defined long/short same-symbol conflict rule before both submit | **yes (new)** | new `MULTI-STRATEGY-CONFLICT-POLICY-01` |
| Broker submit | side-agnostic, unchanged | unchanged initially | no | `mqk-broker-alpaca/src/lib.rs::submit_order` |
| Live mode | hard-blocked (B5 + policy) | hard-blocked **forever** | no | `decision.rs`, `short_entry_policy.rs` |

## 7. Files/functions that MUST change later (in dependency order)

1. `mqk-broker-alpaca/src/lib.rs` — add a **read-only** `GET /v2/assets/{symbol}`
   method feeding `parse_alpaca_asset_json` → `ShortabilityPreflightResult`
   (`ShortabilitySource::AlpacaAsset`). New code; not a change to submit.
2. `capital_policy/short_entry_policy.rs` — add `parse_short_entry_config_from_env`
   (or load from the existing `MQK_CAPITAL_POLICY_PATH` file) so the real
   `ShortEntryConfig` reaches the gate.
3. `decision.rs::bar_result_to_decisions` (or a new gate immediately downstream)
   — replace the unconditional `ShortOpen` drop with: evaluate
   `evaluate_short_entry_policy_with_preflight(...)` and only emit a sell-to-open
   decision when it returns `ShortOpenAllowed`; otherwise keep dropping
   fail-closed. **B5 is not removed — it is conditioned on policy authorization.**
4. A multi-strategy conflict policy (new module) before any concurrent long+short.

## 8. Files/functions that MUST NOT change (to keep the safety contract)

- The live-mode block in `evaluate_short_entry_policy` /
  `_with_preflight` (`live_short_entries_blocked`, before any flag/shortability).
- `mqk-broker-alpaca/src/lib.rs::submit_order` and all broker submit/live-routing
  code (no short-specific submit path).
- `state/dry_run_strategy.rs` signatures (no `AppState`/`PgPool`/broker handle).
- The `fail_closed()` defaults and per-field fail-closed parsing in
  `parse_short_entry_config`.
- DB migrations (append-only; no new migration is needed to *enable* a paper
  short — enablement is policy-file + env + gate-wiring, not schema).

## 9. Required config/env/DB for paper-only short enablement

- `MQK_DAEMON_DEPLOYMENT_MODE=paper`, `MQK_DAEMON_ADAPTER_ID=alpaca` (paper+Alpaca
  is the only credible autonomous path).
- A `capital_allocation_policy.json` (schema `policy-v1`) with a
  `short_entry_policy` section: `allow_short_entries=true`,
  `paper_short_entries_enabled=true`, `live_short_entries_enabled=false`,
  `require_shortable_check=true`, plus `max_short_shares` / `max_short_notional_usd`
  caps. Pointed to by `MQK_CAPITAL_POLICY_PATH`.
- **Coupling note**: introducing that file also activates Gate 1e
  (`evaluate_strategy_budget`). The short strategy then needs an explicit
  `per_strategy_budgets` entry with `budget_authorized=true`, or Gate 1e denies it
  even before the short gate is reached.
- DB: the short strategy id must be **registered and enabled** in
  `sys_strategy_registry` (Gate 3 of `submit_internal_strategy_decision`) and not
  present in `sys_strategy_suppressions` (Gate 4). No new tables/migrations.

## 10. Proof obligations for the first paper short

- **No live routing**: `GET /api/v1/system/status` →
  `daemon_mode="paper"`, `adapter_id="alpaca"`, `live_routing_enabled=false`
  (`routes/helpers.rs`).
- **No dry-run submit after enablement**: the secondary stays in
  `MQK_DRY_RUN_STRATEGY_IDS`; `GET /api/v1/strategy/dry-run/status` rows keep
  `submitted=false`; outbox query for that strategy id stays `0`.
- **A short actually opened safely**: an `oms_outbox` row with
  `order_json->>'side' = 'sell'` opening a net-short position, a broker
  ack/fill via the inbox, and a resulting **negative** `qty_signed` in portfolio.
- **Immediate flatten + reconcile**: buy-to-cover returns the position to flat
  (`scenario_short_side_flatten_proof_01.rs` is the unit proof of cover
  emission); reconcile shows the signed short and then a clean flat with no
  `ReconcileDrift` halt.

## 11. Recommended patch sequence (after Monday dry-run smoke passes)

Do **not** enable paper short entries until all of these hold:
the Monday dry-run market smoke passes; a conflict policy is defined; real or
paper-test shortability proof is wired (mocked only in paper tests); the B5
change is gated behind a paper-only short-entry policy; and live mode remains
hard-blocked.

Safest-first ordering:

1. **`SHORT-SIDE-ALPACA-ASSET-LOOKUP-01`** — add the read-only `GET
   /v2/assets/{symbol}` method + `ShortabilityPreflightResult` construction.
   Read-only, touches no submit/dispatch/B5, removes the biggest unknown
   (shortability) before anything is unblocked. **Recommended first.**
2. `MULTI-STRATEGY-CONFLICT-POLICY-01` — define the long/short same-symbol rule
   (still no submission change).
3. `SHORT-SIDE-B5-PAPER-GATED-BYPASS-01` — wire `parse_short_entry_config` and
   condition the B5 drop on `ShortOpenAllowed` (paper-only), with live still
   hard-blocked. This is the only patch that changes order behavior; it comes
   last and depends on (1) and (2).

`SHORT-SIDE-PAPER-PRIMARY-STRATEGY-SEAM-01` (making the short strategy the
primary submitter) is sequenced with/after step 3 and gated by the conflict
policy.
