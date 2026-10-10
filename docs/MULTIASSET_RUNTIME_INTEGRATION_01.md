# V4-MULTIASSET-RUNTIME-INTEGRATION-01

Mission record for `feature/v4-multiasset-runtime-integration-01`, based on
`origin/main` `da901442`. Scope: contract rows C11-C14 and the A-L asset gates
at the actual execution seams. Local proof only; GitHub CI `DISABLED / NOT RUN`;
no Paper/Live activity, no broker or provider call, no holdout access, no push.
Status words follow `docs/MQD_V4_PERMANENT_COMPLETION_CONTRACT_2026-10-10.md`;
nothing here is `INDEPENDENTLY ACCEPTED` or `OPERATIONALLY VERIFIED`.

## Reused, not rebuilt

The accepted M2 runtime (arbitration required, exactly-once claimed bar), the
M5-M8 asset stack (registry v2, `QtyMicros`, domain-keyed ownership, crypto TIF
policy, options lifecycle, IBKR foundation behind a fake transport) and Bundle
5/6 are unchanged. No scheduler, allocator, netting, leverage or capital-priority
policy was introduced.

## Defect census

| # | Seam | Finding | Disposition |
|---|------|---------|-------------|
| 1 | `decision::submit_internal_strategy_decision` | Active run, intake counters and per-symbol counters were hardcoded to `EquityNyse`, so a registry-resolved Crypto decision was enqueued onto the Equity domain's outbox | FIXED+PROVEN (`bd1995e3`) |
| 2 | same seam, crypto admission | A crypto instrument quoted in a non-account currency was admitted and then sized, capped and allocated against account equity as if it were account-currency | FIXED+PROVEN (`770abd47`) |
| 3 | `POST /api/v1/execution/orders` | Manual order has no `asset_class`; dispatch reads that as Equity; nothing proved the symbol was an Equity, so an OCC option or crypto pair bypassed the per-class broker capability gate (the hole Gate 0b closed for the external signal route) | FIXED+PROVEN (`5ff7b367`) |
| 4 | operator flatten + pre-event flatten | Close orders are Equity-shaped with no `asset_class`; a fractional Crypto position's close was reported "enqueued" and quarantined at dispatch ("must be a whole share count"). First fix (`257de570`) refused only positions registry-v2 listed as non-Equity and treated absence as permission, so an unclassified OCC option or crypto position still got an Equity close | FIXED+PROVEN in two steps: `257de570` (partial) superseded by `2468732d` (positive Equity proof required) |
| 5 | same-symbol arbitration, claimed-bar single evaluation, concurrent claim, stale bar | Accepted M2 controls | ALREADY CORRECT (not reopened) |
| 6 | `Alpaca supports_asset_class`, `validated_asset_class`, outbox parse | Options/futures/forex refused at capability and at the payload parser; crypto flag default-off | ALREADY CORRECT |
| 7 | Bundle 5 capital allocation default `off` | With allocation off there is no MQD-level shared-capital guard across strategies; broker buying power is the only backstop. Enforcing it would need an operator capital policy (and an opportunity artifact) | BLOCKED (operator capital policy), documented limit |
| 8 | account entitlement evidence (`trading_blocked`, `crypto_status`, options level) | Not read anywhere; the broker rejects at submit | BLOCKED (needs real provider response evidence + operator entitlement policy; C25) |
| 9 | futures / forex execution | `mqk-broker-ibkr` has identity/mapping/normalization behind a fake transport and is not a dependency of the daemon; no IB Gateway, API surface or roll policy | BLOCKED (hard stop: provider capability + operator decision) |
| 10 | options entry | Decision seam refuses `option`; mleg adapter capability default-off; REST success path unproven | BLOCKED (operator capability decision) |
| 11 | `asset_class_scope`/static capability matrix report `equity_only`/"no adapter wired" | Correct for the default configuration (all non-equity capabilities default-off); stale prose only | OUT-OF-SCOPE (no false state demonstrated) |
| 12 | `fee_attribution_for_symbol`, `is_alpaca_crypto_symbol` infer crypto from `/` in the symbol | Inbound `BrokerEvent` carries no asset class; a future `EUR/USD` pair would be read as crypto (fails toward partial cost truth) | DEFERRED: needs an asset class on the broker event when a forex adapter exists |
| 13 | `runtime_risk` is account-level (PDT applies to every order) | Over-restrictive for crypto, never fail-open | OUT-OF-SCOPE |
| 14 | Bundle 5 `per_candidate_timeframe_label` keyed by symbol | Unreachable while one timeframe per symbol (M2 note) | DEFERRED (latent) |
| 15 | `scenario_internal_strategy_decision` DB suite | The D3 lifecycle test leaves a pending event that fails later tests on a reused DB (order-dependent fixture hygiene) | OUT-OF-SCOPE, pre-existing |

## Changes

1. `770abd47` `ACCOUNT_CURRENCY` is the single account-currency seam; a crypto
   instrument whose quote currency differs is refused
   (`currency_conversion_unsupported`). The portfolio economics route reads the
   same seam.
2. `bd1995e3` The instrument context is resolved once; the owning domain comes
   from its asset class; Gates 1/1f/6 and the post-enqueue counters use it. A
   Crypto decision with no Crypto run is refused and writes no outbox row.
3. `5ff7b367` Manual orders require the decision seam's registry authority to
   resolve the symbol to an Equity (400 rejected / 503 unavailable).
4. `257de570` (superseded by item 5) Refused a position the trading registry-v2
   listed as non-Equity; absence of a listing was treated as permission.
5. `2468732d` One shared decision, `decide_equity_flatten_close`, returns
   `ProvenEquity` / `NotEquity` / `Unproven`, and both flatten producers create
   a close only for `ProvenEquity`. The canonical Equity registry (loaded,
   validated, every row for the symbol inspected ignoring case) must list the
   symbol exactly once as an enabled Equity; a configured registry-v2 must also
   be readable, valid, free of the test bypass and non-contradictory. Absent,
   unlisted, unreadable, invalid, duplicate or contradictory evidence creates
   zero order intents. The operator route reports refused positions separately
   ("position NOT closed"), keeps proven Equities flowing in a mixed portfolio,
   and writes `FLATTEN_NOT_SUBMITTED` when nothing was enqueued. Consequence:
   a held ticker the canonical registry does not list as an enabled Equity is
   no longer closed by MQD; the warning tells the operator to close it at the
   broker or add a validated registry entry. No broker flag, capital policy or
   Live behavior changed.

## Proof

Each invariant has a DB-backed or unit test on the production seam and a
mutation that turned it RED, restored byte-identically (SHA-256 checked):
MD-01/02 (domain derivation), the currency unit test (check disabled), MO-01/02/04
(identity gate disabled), FP-04..10 (positive-proof gate failing open), FP-07 (registry-v2
validation removed), FP-03..09 (operator caller ignoring the decision), FP-10 (pre-event
caller ignoring it), FP-11 (canonical disabled/non-Equity guards removed), and the audit
label. The first-step FL-02/03/05 mutation is superseded. Controls: MD-03, MO-03,
FL-04.

## Extensibility (operator clarification)

No US-only or USD-only assumption was added. The currency check compares
against one replaceable seam and fails closed; an unregistered class or venue
has no domain mapping and is refused at Gate 7, never routed by default.
Pre-existing closed-universe assumptions found and left unchanged: the
two-variant `ExecutionDomain`, the equity-only control plane routes, the static
`asset_class_scope` constant, and finding 12.

## Asset-class gates (A-L), software state at this HEAD

`IMPL` = code and tests exist; `BLOCKED` = needs an operator decision or
provider capability; `UNV` = not verified here. No cell is `OPERATIONALLY VERIFIED`.

| Class | A | B | C | D | E | F | G | H | I | J | K | L |
|-------|---|---|---|---|---|---|---|---|---|---|---|---|
| Stocks / ETFs | IMPL | IMPL | IMPL | IMPL | UNV | UNV | IMPL (USD only; allocation off by default) | IMPL | IMPL | UNV | BLOCKED | IMPL |
| Crypto spot | IMPL | UNV | IMPL | IMPL (explicit size) | UNV | UNV | IMPL (this mission: currency + domain) | IMPL (this mission: domain, flatten) | BLOCKED (capability flag default-off) | BLOCKED (never run) | BLOCKED (Paper-locked) | UNV |
| Listed options | IMPL (OCC identity, lifecycle) | UNV | UNV | UNV | UNV | UNV | UNV | partial (lifecycle fence) | BLOCKED (mleg default-off) | BLOCKED | BLOCKED | UNV |
| Futures | IMPL (identity) | UNV | UNV | UNV | UNV | UNV | BLOCKED (margin policy) | UNV | BLOCKED (no transport) | BLOCKED | BLOCKED | UNV |
| Spot forex | IMPL (identity) | UNV | UNV | UNV | UNV | UNV | BLOCKED (FX/leverage policy) | UNV | BLOCKED (no transport) | BLOCKED | BLOCKED | UNV |

## Not performed

`cargo test --workspace`, GitHub CI, any Paper/Live order, any broker/provider
call, push. Proofs use a disposable Postgres database only.

## Acceptance boundary (local, disposable Postgres :5434)

`cargo test -p mqk-daemon --lib`: 1159 passed, 0 failed, 22 ignored. Affected
integration suites (the three `scenario_multiasset_*` files, decision
idempotency, fleet enable/disable, manual order submit, paper flatten,
pre-event flatten, live-shadow flatten/no-order, ops control, GUI contract gate,
session hygiene, option lifecycle pending gate, route contract): all green with
`--include-ignored --test-threads=1` on a fresh database. `cargo clippy -p
mqk-daemon --lib --tests -- -D warnings` clean; `git diff --check` clean.
The hermetic manual-order fixture needed the canonical registry anchored
(`5de9318d`) because finding 3 correctly requires a registry-proven Equity.
`cargo test --workspace` was NOT run (laptop resource rule); broad workspace
proof is delegated to GitHub CI, which is disabled here.

## Independent-review correction (flatten positive proof, `2468732d`)

Review of `e48feb89` confirmed one safety blocker: `flatten_refusal_for_symbol`
returned `None` when registry-v2 was absent, unreadable or silent about a symbol,
and both flatten producers read `None` as permission. Corrected as described in
change 5. Reproduced RED before the fix with the route-level matrix (FP-04..09
failed against `e48feb89`; FP-01..03 controls passed).

Proof: `scenario_multiasset_flatten_equity_proof_01` FP-01..FP-11 (operator route
and the automatic pre-event path), the first-step `scenario_multiasset_flatten_position_class_01`
(FL-01..05, updated to the new API), and seven guard-removal mutations (positive-proof
gate, registry-v2 validation, canonical disabled and non-Equity guards, each
production caller, and the audit label), each killed and restored byte-identically.
Fixtures that call the changed routes now anchor the canonical registry
(`scenario_paper_flatten_psf01`, `scenario_liveshadow_no_order_authority_01`).

Affected acceptance set (one combined run, fresh disposable DB,
`--include-ignored --test-threads=1`): the two flatten files, the other two
`scenario_multiasset_*` files, `scenario_paper_flatten_psf01` (11),
`scenario_pre_event_flatten_01` (24), `scenario_liveshadow_no_order_authority_01`,
`scenario_live_shadow_flatten_on_halt_01`, `scenario_ops_control_oc01_oc02` (16),
`scenario_autonomous_paper_session_hygiene_01` (11), `scenario_gui_daemon_contract_gate` (23),
`scenario_daemon_order_submit` (20), `scenario_route_contract_rt01`: all passed.
Touched-module lib tests (`decision::`, `loop_runner::`, `hermetic_`, `pre_event`,
`control_plane`): 57 passed, 10 ignored. Clippy `-D warnings` clean. The full 1,159-test
daemon lib suite and the 22 `#[ignore]` lib tests were not rerun (no change reaches them),
and `scenario_internal_strategy_decision` keeps its documented reused-DB limitation
and was not rerun.

Still open (unchanged by this correction): findings 7-10 and 12-14 above;
the operator must close an unproven held position at the broker or add a validated
registry entry; nothing here proves any asset class operational.
