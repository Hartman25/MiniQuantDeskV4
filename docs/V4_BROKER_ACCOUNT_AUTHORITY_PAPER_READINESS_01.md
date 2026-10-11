# V4-BROKER-ACCOUNT-AUTHORITY-PAPER-READINESS-01

Mission record for `feature/v4-broker-account-paper-readiness-01` (baseline
`ab9f9ab0a4b7b309924512c023adb65b84fcc812` = `origin/main` at start). Scope:
contract rows C13, C14, C15, C20, C21, C25, C26 for the US equity/ETF Alpaca
Paper lane. Local proof only; GitHub CI not run; no push; no broker orders; no
Live or Paper activity of any kind.

Evidence classes: `CODE/TEST` (offline production-seam proof), `DB-BACKED`
(scratch Postgres on the disposable test server), `PROVIDER` (real provider
read), `PAPER OP` (real Paper lifecycle). Only `CODE/TEST` and `DB-BACKED`
evidence exists for this mission. No `PROVIDER` or `PAPER OP` evidence was
produced; every provider wire body in a test is an in-process fixture of the
documented `GET /v2/account` shape and proves plumbing, never that a real
account is entitled.

## 1. Authority design

Evidence is the typed projection of `GET /v2/account`
(`AccountEntitlementEvidence`, `mqk-broker-alpaca::account_entitlement`).
Provider contract used (Alpaca Trading API `getAccount`): `id`, `status`,
`trading_blocked`, `account_blocked`, `trade_suspended_by_user`,
`crypto_status`, `options_trading_level`, `shorting_enabled`. The provider
documents only `id` and `status` as required, so every other field is treated as
optional on the wire: an absent or wrongly typed field is *unknown* and never an
implicit grant. `status` / `crypto_status` are accepted only as exactly
`ACTIVE`.

The facts stay distinct and are conjunctive at the gateway:

| Fact | Authority |
|---|---|
| Supported by adapter | `BrokerAdapter::supports_asset_class` |
| Enabled by operator | `AlpacaConfig` capability flags (default off; the daemon never sets crypto/mleg true) |
| Permitted by broker/account | `BrokerAdapter::admit_account_entitlement` (this mission) |
| Authorized by deployment mode | parsed Paper host binding + durable account registry (this mission) |
| Proven fresh and available | `AccountEvidenceCell`, age bound = account-risk freshness bound (61 s) |
| Operationally verified | NOT established (needs real Paper evidence) |

Enforcement: `BrokerGateway::submit_with_context` calls
`admit_account_entitlement(Some(asset_class))` after the capability gate and
before integrity/risk/reconcile gates; `replace` calls it with `None`; `cancel`
is intentionally unchanged. There is no risk-reducing exemption: a provider that
reports the account blocked refuses closes too, and MQD does not invent an
exemption. `DaemonBroker` (the production gateway broker) forwards the method; a
unit test fails if the forward is removed. `submit_vertical_spread`, the one
Alpaca submit path outside the gateway (default-off, no production caller), has
the same admission plus `options_trading_level >= 3`.

Caller paths (traced): strategy decision, manual order, flatten, pre-event
flatten and control-plane orders all enqueue to the durable outbox
(`outbox_enqueue_*`); `mqk-runtime::orchestrator::dispatch` is the only caller of
`gateway.submit_with_context`. Every Alpaca HTTP mutation is either that trait
submit/cancel/replace or `submit_vertical_spread`.

Evidence is process-local. A restarted daemon has none and admits nothing until
a fresh `GET /v2/account` is observed (run-start probe or the 60-tick refresh).
If a refresh fails, evidence goes stale within 61 s exactly when the account-risk
snapshot does, and every submit (including risk-reducing) is refused; this
mirrors the pre-existing risk-gate behavior for stale account truth.

## 2. Run-start binding

`build_execution_orchestrator` performs one account probe. For Paper, the
provider account id is registered in `sys_broker_account_authority` (an account
already registered under a Live mode refuses the start,
`runtime.start_refused.broker_account_identity_conflict`). Live modes are not
registered here: a one-mode-per-account registry would block the shadow ->
capital transition on the same live account. The run's admission is pinned to
the account id, so a later observation naming another account (for example the
snapshot fetcher and the execution adapter built from different environments)
is refused as `account_identity_drift`. A definite provider denial
(`status != ACTIVE`, `trading_blocked`, `account_blocked`,
`trade_suspended_by_user`) refuses the start with
`runtime.start_refused.broker_account_not_entitled` (surfaced by the lifecycle as
its existing internal-error class with that `fault_class`). A failed probe or
unknown fields never refuse the start; they keep submissions refused.

## 3. Environment identity

`ALPACA_PAPER_BASE_URL` was accepted verbatim. Paper now requires the parsed
https host `paper-api.alpaca.markets` (loopback only for hermetic mocks); the WS
transport uses the same authority; the Paper-only crypto gate matches the parsed
host rather than a substring.

## 4. Readiness

`GET /api/v1/system/preflight` gains `broker_account_entitlement`, computed by
the same `admit` logic as the gateway (`not_observed | entitled | denied | stale
| unknown`). Preflight and `GET /api/v1/autonomous/readiness` share
`AppState::broker_start_blockers` (non-Paper Paper endpoint override; fresh
provider denial), appended after existing gates. `not_observed`, `stale` and
`unknown` are warnings because a start probes the account afresh. A preflight
warning states the consequence of more than one configured strategy with the
shared-capital allocator not enforcing.

## 5. Durable provenance and currency

Migration 0092 adds the set-once `sys_paper_portfolio_snapshot_account`; a
confirmed Paper snapshot is bound only when the evidence cell's latest
observation is the very `GET /v2/account` that captured it and names the pinned
account. Unproven snapshots stay account-unverified (no inference, no backfill).
`DaemonAccountAuthority` refuses account equity whose currency is not USD (all
risk limits and baselines are USD).

## 6. Commits (branch `feature/v4-broker-account-paper-readiness-01`)

| Commit | Invariant |
|---|---|
| `114f95f7` | Paper environment identity = parsed host. NOT independently buildable: a hunk-split staging error placed helpers inside `impl AlpacaConfig`; repaired in `851d336b` (history not rewritten: rebase/reset prohibited). |
| `851d336b` | gateway + adapter account-entitlement authority (library) |
| `a5449b74` | DaemonBroker forwarding, shared evidence cell, run-start probe/registry/pin/refusal |
| `61a4a294` | preflight `broker_account_entitlement` |
| `f6e772a2` | shared broker start blockers in both readiness routes |
| `7fc240d4` | non-USD account equity refused by the risk authority |
| `95d81053`, `95cf0816` | durable snapshot -> provider account binding (migration 0092, LF pin) |
| `9bcd20c9` | options spread submission requires provider spread entitlement |
| `60f2fe24` | real-orchestrator dispatch proof (DB) + entitlement refusal logged at dispatch |
| (this record) | mission record |

## 7. Proof

Pre-fix RED is not reproducible for new capabilities (no prior seam existed); the
substitute is guard-removal mutation, each restored byte-for-byte (sha256 prefix
verified) and the tree clean afterwards.

| Mutation | Killed by |
|---|---|
| host check reverted to substring | `paper_host_must_be_parsed_host_not_substring` |
| gateway entitlement call removed | `e01`, `e03`, `e04` (`scenario_account_entitlement_gate_01`) |
| evaluator `trading_blocked` check removed | 4 tests incl. `each_blocked_or_unknown_field_refuses_with_its_code` |
| freshness check removed | `stale_and_future_observations_refuse`, `stale_observation_cannot_admit` |
| identity-pin check removed | `pinned_account_identity_drift_refuses` |
| adapter ignores evidence | 5 adapter tests |
| unbound adapter admits | `unbound_adapter_never_admits` |
| DaemonBroker forward removed | `alpaca_variant_forwards_account_entitlement_authority`, `real_gateway_refuses_unentitled_account_before_any_http` |
| Paper endpoint validation removed | `paper_base_url_override_must_target_paper_host_or_loopback` |
| snapshot capture-instant check removed | `binds_only_when_the_capturing_observation_proves_the_pinned_account` |
| non-USD check removed | `external_non_usd_account_currency_is_malformed_never_assumed_usd` |
| Paper registry binding removed | `start_refused_when_account_is_registered_under_another_deployment_mode`, `start_probes_account_registers_identity_and_binds_admission` |
| definite-denial start refusal removed | `start_refused_when_provider_reports_trading_blocked` |
| run pin removed | `start_probes_account_registers_identity_and_binds_admission` |
| `denied` start blocker removed | `broker_environment_start_refusals_are_reported_by_both_readiness_routes` |
| snapshot pin check removed | `unproven_account_leaves_the_snapshot_account_unverified` |
| mleg entitlement gate removed | `mleg_flag_alone_is_not_provider_permission` |
| gateway entitlement call removed (real orchestrator + DB outbox) | `d1_refusing_entitlement_fails_the_outbox_row_and_never_reaches_the_broker` (`scenario_account_entitlement_dispatch_01`) |

DB-backed tests are `#[ignore]` (loud on a missing URL) and were run against a
scratch database on the disposable test server only. They are not yet registered
in the CI DB proof lane (CI is disabled during development).

Environment notes: `MQK_RISK_INITIAL_EQUITY_USD`, `MQK_RISK_DAILY_LOSS_LIMIT`,
`MQK_RISK_MAX_DRAWDOWN` must be set for the autonomous-task DB scenarios to
reach `Started` (pre-existing requirement; without them they refuse with
`runtime.start_refused.portfolio_seed_missing`).

## 8. Defect census

| # | Finding | Disposition |
|---|---|---|
| 1 | Account fields (`trading_blocked`, `account_blocked`, `status`, `trade_suspended_by_user`, `crypto_status`, options level) never read | FIXED+PROVEN |
| 2 | Config flag the only crypto/mleg authority | FIXED+PROVEN (admission is conjunctive; mleg gated on provider level) |
| 3 | Paper endpoint override accepted any host (Paper label on Live API) | FIXED+PROVEN |
| 4 | Crypto Paper-only gate matched host by substring | FIXED+PROVEN |
| 5 | Credential/account mixing between adapters of one process | FIXED+PROVEN (pin + drift) |
| 6 | Paper start against an account registered as Live | FIXED+PROVEN (DB) |
| 7 | Readiness green while start would refuse: entitlement denial, Paper endpoint | FIXED+PROVEN (both routes) |
| 8 | Readiness green while start would refuse: missing Alpaca credentials (`broker_config_present` is adapter-id based) | OPEN, deferred: pre-existing, outside the entitlement invariant; every readiness fixture lacks credentials so a blocker would churn them |
| 9 | Stale/missing evidence authorizing orders | FIXED+PROVEN; evidence not persisted across restart (ALREADY CORRECT by design, `p01`, failed-probe lifecycle test) |
| 10 | Non-USD account equity feeding USD risk limits | FIXED+PROVEN |
| 11 | Durable snapshots not attributable to a provider account | FIXED+PROVEN (DB) |
| 12 | Cancel after entitlement loss | ALREADY CORRECT: unchanged; still subject to integrity/risk/reconcile gates (risk gate denies on stale account); `e07` |
| 13 | Verified risk-reducing close while the account is blocked | REFUSED, no exemption invented. BLOCKED on provider evidence: whether Alpaca accepts closes from a `trading_blocked` account is not documented; any exemption is an operator/provider decision |
| 14 | Manual/internal/outbox paths bypassing the guard | ALREADY CORRECT+PROVEN: single dispatch call site traced; `e01..e07`, real-gateway test |
| 15 | Unsupported crypto/options/futures/forex | ALREADY CORRECT+PROVEN (capability gate unchanged; new `account_entitlement_unsupported_asset_class`) |
| 16 | Reconciliation drift blocks unsafe activity | ALREADY CORRECT (`ReconcileGate`; existing `scenario_gateway_no_bypass`, `scenario_stale_reconcile_blocks_submit`); not re-proven here |
| 17 | Provider failure/restart truthful readiness | FIXED+PROVEN (`not_observed` after restart; failed probe fabricates nothing) |
| 18 | Shared-capital allocator off with several strategies | EXPOSED (preflight warning). BLOCKED on operator economic-policy decision; allocator not enabled |
| 19 | Operator-pinned expected account id | BLOCKED on operator decision: a different account of the same mode is accepted by the registry (a first-seen account registers) |
| 20 | Live accounts: no registry registration / no probe authorization | OUT-OF-SCOPE; admission applies to Live adapters the same way but is unverified |
| 21 | Real status values for a Paper account (e.g. `PAPER_ONLY` is in the provider enum) | BLOCKED on provider evidence: only `ACTIVE` is accepted; a read-only Paper probe is required to confirm |
| 22 | Dispatch marks the outbox row FAILED on an entitlement refusal and records no durable per-refusal audit row (RiskBlocked has `capture_risk_denial`) | PARTIAL: refusal code/detail now logged (`exec_submit_refused_account_entitlement`, unasserted) and in the tick error (asserted by `d1`); a durable operator-visible refusal record remains OPEN (C26 gap) |
| 23 | Provider rejects, throttling, ambiguous acknowledgements | ALREADY CORRECT (existing `BrokerError` classification and halt paths); not modified |
| 24 | GUI types for `broker_account_entitlement` | DEFERRED (additive field; GUI is another lane) |
| 26 | `state::lifecycle::explicit_multi_strategy_start_snapshot_tests::c3_01_real_registry_promotion_and_evaluate_candidate_drive_durable_authority` (ignored, DB) fails with `RowNotFound` | PRE-EXISTING, unrelated: identical failure on baseline `ab9f9ab0` in a temporary worktree against the same scratch DB; not investigated (outside scope; may depend on shared test-DB state) |
| 25 | Pre-existing tests that silently skip without a DB (e.g. durable snapshot persistence) | INFORMATIONAL; new DB tests fail loudly instead |

## 9. Contribution to the permanent matrix (no status changed)

C13/C14: gateway admission + dispatch-path refusal tests (CODE/TEST). C15:
unchanged, PARTIAL: no real Paper lifecycle. C20: Paper endpoint binding;
no secrets read, persisted or printed. C21: restart-safe evidence (none
persisted) and DB-backed start refusals. C25: explicit provider-entitlement
evidence and denial cases (CODE/TEST + DB); `PROVIDER` read-only account
evidence still missing. C26: durable snapshot -> account provenance.

## 10. Unresolved operator/provider decisions

1. Operator capital-allocation policy before enabling the shared-capital
   allocator (findings 18).
2. Whether to pin an expected Paper account id (finding 19).
3. Read-only Paper account capability probe (credentials, operator permission)
   to confirm the real `status`/flag values (finding 21) and whether closes are
   accepted while `trading_blocked` (finding 13).
