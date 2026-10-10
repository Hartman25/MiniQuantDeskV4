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
| 4 | operator flatten + pre-event flatten | Close orders are Equity-shaped with no `asset_class`; a fractional Crypto position's close was reported "enqueued" and quarantined at dispatch ("must be a whole share count") | FIXED+PROVEN (`257de570`): refused and reported |
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
4. `257de570` A position the trading registry-v2 lists as non-Equity is refused
   by both flatten producers and reported (`unsupported_position`). Equity
   closes, and any symbol the registry does not list or cannot read, keep the
   historical close, so Equity liquidation never depends on registry-v2.

## Proof

Each invariant has a DB-backed or unit test on the production seam and a
mutation that turned it RED, restored byte-identically (SHA-256 checked):
MD-01/02 (domain derivation), the currency unit test (check disabled), MO-01/02/04
(identity gate disabled), FL-02/03/05 (refusal disabled). Controls: MD-03, MO-03,
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
