# REGISTRY-V2-TRANSLATION-01A — Symbol-Keyed Consumer Audit

Patch ID: `REGISTRY-V2-TRANSLATION-01A-SYMBOL-KEYED-CONSUMER-AUDIT-01`

Docs-only. No code changed by this patch. Written to ground
`ASSET-CORE-01H`'s production-cutover prerequisite #2 (an explicit
`instrument_id`/symbol translation layer) in current repo evidence before
any translation code is written in Phase B.

Baseline commits this audit builds on: `2da681e9` (roadmap reconcile),
`f0ce4fff`/`b870c823`/`e3f2f77e` (backtest economics closure —
`BACKTEST-MULTIPLIER-MARGIN-01`, prerequisite #1, now `CLOSED_LOCAL /
BACKTEST-COMPLETE`), `6e6f69df`/`91cc08e1` (`ASSET-CORE-01` foundation
audit + consumption-boundary decision).

---

## 1. Current production symbol-string-keyed surfaces

| Surface | Evidence | Key shape |
|---|---|---|
| `md_bars` | `core-rs/crates/mqk-db/src/md.rs::ProviderBar { symbol: String, ... }`, `mqk_db::md::latest_stored_bar_end_ts(&pool, sym, tf)` | bare ticker string, e.g. `"AAPL"`, matches v1 `equities.json`'s `symbol` field exactly |
| OMS/outbox rows | `core-rs/crates/mqk-db/migrations/0001_init.sql`: `oms_outbox(outbox_id, run_id, idempotency_key, order_json jsonb, status, ...)` | no dedicated `symbol` column — the symbol string is embedded inside `order_json`'s serialized order payload, not a queryable DB column |
| Portfolio positions/snapshots | `core-rs/crates/mqk-portfolio/src/types.rs::PortfolioState.positions: BTreeMap<String, PositionState>`, `PositionState.symbol: String`; `core-rs/crates/mqk-reconcile/src/types.rs::PositionSnapshot { symbol: String, qty_signed: i64 }`; `core-rs/crates/mqk-runtime/src/observability.rs::PositionSnapshot` (mirrors the same shape) | bare ticker string keys throughout |
| Backtest/bar lookup surfaces | `core-rs/crates/mqk-cli/src/commands/md.rs::resolve_symbols`, `md_ingest_provider`, `md_sync_provider` — all take `--symbols`/`--symbols-from-registry` as bare-ticker strings and read/write `md_bars` by that string | same bare-ticker keyspace as `md_bars` |

All five surfaces share one keyspace: the bare ticker string exactly as it
appears in `config/instruments/equities.json`'s `symbol` field (v1
`TrackedInstrument`). None of them read `instrument_id`, `asset_class`, or
any other `InstrumentRegistryV2` field today.

## 2. Current registry-v2 identity fields

From `core-rs/crates/mqk-md/src/instrument_registry_v2.rs::InstrumentDefinitionV2`:

- `instrument_id: String` — globally unique, e.g. `"equity:US:AAPL"`.
- `symbol: String` — "primary human-facing symbol", e.g. `"AAPL"`, `"BTC/USD"`.
- `asset_class: String` — one of `CANONICAL_ASSET_CLASSES_V2` (`equity`,
  `option`, `future`, `crypto`, `forex`, `rate`).
- `instrument_kind: Option<String>` — e.g. `Some("etf")`.
- `provider_symbols: BTreeMap<String, String>` — provider name -> provider-
  specific symbol.

`validate_registry_v2` already fail-closed rejects duplicate `instrument_id`
and duplicate `symbol` within one registry document (`v2_04`/`v2_05` tests),
and empty `instrument_id`/`symbol`/`asset_class`/`currency`.

## 3. Current v1 registry identity fields

`config/instruments/equities.json` (`TrackedInstrument`, v1) rows carry
`instrument_id` (`"equity:US:{SYMBOL}"`), `symbol` (bare ticker), `provider`,
`provider_symbol`, `venue`, `currency`, `enabled`, `timeframes`, and
optionally `instrument_kind`/`sector`/`category` for ETFs.

Direct inspection of the current 88-row file confirms:
- **0 duplicate `symbol` values.**
- **0 duplicate `instrument_id` values.**
- **14 rows** carry `instrument_kind: "etf"` (e.g. `SPY`, `QQQ`, `DIA`, `TLT`,
  `XLE`..`XLV` sector ETFs), all with `asset_class: "equity"` and a bare
  ticker `symbol` in the exact same keyspace as non-ETF equities — no
  separate ETF symbol format exists in this file.

## 4. Current v1 -> v2 conversion behavior

`convert_tracked_instrument_to_v2` (`instrument_registry_v2.rs`) copies
`instrument_id` and `symbol` verbatim from the v1 row — it does not
transform, re-derive, or re-format either field. `provider_symbols` is
populated from the single `(provider, provider_symbol)` pair v1 already
proves. `convert_v1_registry_to_v2` maps the full slice 1:1, preserving
order and count. Because the copy is verbatim and v1 already has zero
duplicate `symbol`/`instrument_id` values, converting the current
production universe through this function cannot introduce a collision
that did not already exist in `equities.json` itself.

## 5. Collision risks

| Risk | Current-repo status |
|---|---|
| Duplicate legacy symbol | None found in the current 88-row equity universe (§3). `validate_registry_v2` also fail-closed rejects this at the v2 layer if it ever occurred. |
| Duplicate instrument_id | None found in the current 88-row equity universe (§3); same fail-closed check exists in `validate_registry_v2`. |
| Symbol case | All 88 v1 symbols are already upper-case tickers; no case-folding collision is currently possible in the equity universe. A translation layer should still normalize case explicitly rather than assume this holds forever. |
| Slash/pair symbols (e.g. `BTC/USD`) | Only appear in v2 crypto/forex fixtures (`config/instruments/instruments_v2.crypto_local_marks.example.json`), which are `enabled: false` and never touch `md_bars`/outbox/positions in production. No current collision with any bare-ticker equity symbol. |
| ETF-as-equity representation | ETFs share the equity `asset_class` and bare-ticker `symbol` keyspace (§3) — no separate namespace, so no additional collision surface beyond what plain equities already have. |
| Provider alias ambiguity | `provider_symbols` is a `BTreeMap<provider, provider_symbol>` — today every converted v1 row has exactly one entry (its own `provider`/`provider_symbol` pair). Multi-provider alias collision (same provider string mapping to two different canonical symbols) is not currently exercised anywhere in the repo and is out of scope for this patch; recorded as future work in §8 below. |

## 6. Required translation contract

The next safe prerequisite is a **pure, in-memory** lookup layer providing:

1. `instrument_id -> legacy symbol` (the `md_bars`/outbox/positions key).
2. `legacy symbol -> instrument_id`.
3. `canonical symbol -> instrument_id` (for the converted v1 equity
   universe, canonical symbol and legacy symbol are the same string, so this
   is the identity relation composed with #2; kept as a distinct lookup so
   the contract also holds for a future registry where canonical symbol and
   legacy symbol diverge, e.g. pair-style crypto symbols).
4. `canonical symbol -> legacy symbol`.
5. Provider alias (`provider_symbols` value) -> canonical symbol is **not**
   included in this patch's contract — the current repo has no consumer
   that needs it, and building it now would be speculative (see §5's
   provider-alias-ambiguity row and §8).

## 7. What this patch proves

- Every current symbol-string-keyed production consumer relevant to
  `ASSET-CORE-01H` prerequisite #2 is named with direct source/schema
  evidence (§1).
- The exact `InstrumentRegistryV2`/v1 identity fields available for
  translation are named (§2-§4).
- The current equity universe (88 rows) has zero symbol/instrument_id
  collisions, confirmed by direct inspection of `equities.json` (§3), which
  is the concrete precondition Phase B's fail-closed index needs to build
  successfully today.
- The minimal translation contract Phase B must implement is named (§6),
  scoped to only what a current consumer could need.

## 8. What this patch does not do

- **No production cutover.** No file in `core-rs/crates/mqk-runtime`,
  `mqk-execution`, `mqk-risk`, `mqk-broker-alpaca`, `mqk-db` migrations, or
  `mqk-portfolio/src/accounting.rs` is touched by this patch or any phase of
  this bundle.
- **No DB schema change, no migration.** This patch and the ones that follow
  it in this bundle read only local JSON fixtures/config files.
- **No trading enablement.** No config flag is changed; no asset class is
  newly enabled; BTC/USD and ETH/USD remain `enabled: false` wherever they
  are defined.
- **No provider-alias translation layer.** Recorded above (§5, §6.5) as
  explicitly out of scope — a future patch may add it if a real consumer
  needs it.
- **No replacement of `config/instruments/equities.json`.** It remains the
  sole registry any trading/ingestion/backtest/GUI path reads.

---

**Status:** Audit only. Does not itself close `ASSET-CORE-01H` prerequisite
#2 — that requires Phase B's pure lookup layer and Phase C's validator
proof, both gated on this audit's findings holding up.
