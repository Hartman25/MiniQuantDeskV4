# REGISTRY-V2-TRANSLATION-01D — Closure Decision

Patch ID: `REGISTRY-V2-TRANSLATION-01D-CLOSURE-AND-ROADMAP-RECONCILE-01`

Decision-only. No code changed by this patch. Written after `01A` (audit),
`01B` (pure translation index), and `01C` (read-only CLI/report proof) to
decide whether `ASSET-CORE-01H`'s production-cutover prerequisite #2 is
closed and to reconcile the roadmap accordingly.

---

## 1. Is prerequisite #2 of `ASSET-CORE-01H` closed?

**Yes, for the additive translation-layer foundation.** Prerequisite #2
reads:

> An explicit `instrument_id`/symbol translation or lookup path between
> `InstrumentRegistryV2` and every existing symbol-string-keyed table
> (`md_bars`, outbox rows, portfolio positions), proven idempotent and
> collision-free against the current equity universe.

`01B` built exactly that path (`RegistryV2SymbolTranslationIndex`), and
`01C` proved it end-to-end: the full 88-row converted v1 equity universe
builds collision-free and every instrument round-trips
(`legacy symbol -> instrument_id -> legacy symbol`), which is the
concrete acceptance bar `ASSET-CORE-01H` §8 named ("A symbol-translation
round-trip test ... with zero collisions across the full production
instrument set"). The build is a pure function of its input — the same
registry always produces the same index (no wall-clock, no randomness, no
IO beyond the initial file read) — which is what "idempotent" means for a
construction function with no side effects to repeat.

## 2. What translation paths now exist?

All four named by the `01A` audit's translation contract, implemented in
`RegistryV2SymbolTranslationIndex` (`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`):

1. `instrument_id -> legacy symbol` — `instrument_id_to_legacy_symbol`.
2. `legacy symbol -> instrument_id` — `legacy_symbol_to_instrument_id` (case-insensitive).
3. `canonical symbol -> instrument_id` — `canonical_symbol_to_instrument_id`.
4. `canonical symbol -> legacy symbol` — `canonical_symbol_to_legacy_symbol`.

All four are proven via `01B`'s 11 unit tests and `01C`'s 8 CLI scenario
tests (`mqk md registry-v2-translation-check`).

## 3. What surfaces remain symbol-string-keyed?

Every one named by the `01A` audit remains untouched and symbol-string-keyed
today: `md_bars` (`mqk_db::md::ProviderBar.symbol`), `oms_outbox`
(symbol embedded in `order_json`, no dedicated column),
`mqk-portfolio`/`mqk-reconcile`/`mqk-runtime` `PositionSnapshot`/
`PortfolioState` positions, and the backtest/bar-lookup CLI surfaces
(`resolve_symbols`, `md_ingest_provider`, `md_sync_provider`). This patch
adds a lookup path *between* `InstrumentRegistryV2` and those surfaces — it
does not change any of their internal representations.

## 4. Did any production path start consuming v2?

**No.** `RegistryV2SymbolTranslationIndex` has exactly two callers in the
current repo: its own unit tests (`01B`) and the read-only
`registry-v2-translation-check` CLI command (`01C`). Neither `mqk-runtime`,
`mqk-execution`, `mqk-risk`, `mqk-broker-alpaca`, any DB migration, nor
`mqk-portfolio/src/accounting.rs` was touched by `01A`-`01D`. `config/instruments/equities.json`
remains the sole registry any trading/ingestion/backtest/GUI path reads.

## 5. What was proven collision-free?

The full current 88-row `config/instruments/equities.json` universe,
converted to `InstrumentRegistryV2` via the existing (unmodified)
`convert_v1_registry_to_v2`, builds a `RegistryV2SymbolTranslationIndex`
with zero duplicate-`instrument_id` and zero duplicate-canonical-symbol
violations — confirmed both by `01B`'s `trans01` unit test and `01C`'s
`rvtc_01` CLI scenario test (`converted_v1_instrument_count=88`,
`all_passed=true`). Separately, the committed disabled BTC/USD+ETH/USD
crypto v2 fixture (`config/instruments/instruments_v2.crypto_local_marks.example.json`)
also builds collision-free on its own (`01C`'s `rvtc_02`), independent of
the v1 lane — the two are never merged into one index.

## 6. What was proven idempotent/round-trippable?

Every one of the 88 converted v1 instruments round-trips exactly:
`legacy_symbol_to_instrument_id(symbol)` followed by
`instrument_id_to_legacy_symbol(instrument_id)` returns the original
symbol string unchanged, and the reverse composition holds too — proven for
all 88 rows by `01C`'s `rvtc_01` (`round_trip_checked_count=88`) and
individually for `AAPL` by `01B`'s `trans02` unit test. ETF symbols (`SPY`,
`TLT`) additionally preserve their `instrument_kind="etf"` metadata through
the same lookup (`01B`'s `trans03`). Idempotency of *construction* (the
same input registry always yields the same index, proven by `01B`'s
`trans11` deterministic-ordering test) is what "idempotent" means here,
since `build()` has no side effects to repeat non-idempotently — this
differs from, and does not substitute for, the DB-write idempotency
`db_rules.md` requires of `outbox_claim_batch`-style write paths, none of
which this patch touches.

## 7. Are crypto v2 fixture symbols included? If yes, how are they treated?

Yes, as a second, **independent** translation index — never merged with
the equity lane, never implying production readiness. The `01C` CLI's
`--registry-v2` path builds a translation index for the crypto fixture,
reports its two rows as `non_tradable_count=2`/`enabled_count=0`
(`01C`'s `rvtc_02`), and preserves the pair-style symbol `"BTC/USD"` exactly
(slash untouched) rather than reformatting it to fit the equity bare-ticker
convention (`01B`'s `trans08`). If either crypto row's
`paper_trading_enabled`/`live_trading_enabled` flag were ever flipped
`true`, the CLI fails closed with `truth_state=unsafe_trading_enabled`
(`01C`'s `rvtc_05`) rather than silently reporting it as translated and
safe.

## 8. What remains before production cutover?

Prerequisite #2 (this bundle) is now closed. The other four prerequisites
from `ASSET-CORE-01H` §5 remain open, unchanged by this bundle:

1. ~~`BACKTEST-MULTIPLIER-MARGIN-01` closed~~ — already satisfied
   (`BACKTEST-MULTIPLIER-MARGIN-01`, `CLOSED_LOCAL / BACKTEST-COMPLETE`).
2. ~~Symbol/`instrument_id` translation layer~~ — **now satisfied by this
   bundle (`01A`-`01D`).**
3. Gate 0 and the broker-submit routing guard re-verified against
   `InstrumentRegistryV2::asset_class` parity — still open.
4. At least one non-equity market-data provider live-network-verified
   end-to-end into `md_bars` — still open (requires a live network call,
   forbidden by this session's hard safety rules).
5. An explicit operator decision to enable `enabled=true` for a specific,
   named non-equity instrument — still open.

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains blocked on
prerequisites #3-#5 and is **not** recommended next.

## 9. What next patch is recommended?

**`REGISTRY-V2-GATE-PARITY-01`** — prerequisite #3, a regression-test-only
patch proving Gate 0 and the broker-submit routing guard reject the same
set of asset classes whether keyed off `mqk_schemas::AssetClass` (today's
truth) or `InstrumentRegistryV2::asset_class` (the parallel v2 field). This
is the next cheapest prerequisite: it requires no new runtime behavior,
only a parity proof against code that already exists, matching the
value-per-risk pattern every closed sub-slice in this roadmap has followed.
Prerequisites #4 (live non-equity provider verification) and #5 (explicit
operator enablement) remain **not** recommended next — #4 requires a live
network call (forbidden this session), and #5 is an operator decision that
should not precede #3-#4 being settled.

---

## Closure decision

```text
Prerequisite #2 of ASSET-CORE-01H's production-cutover checklist is
CLOSED_LOCAL for the additive translation-layer foundation.
No production cutover occurred. Existing symbol-string-keyed production
surfaces (md_bars, oms_outbox, portfolio positions) remain unchanged.
REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01 is still blocked on
prerequisites #3 (Gate 0/routing-guard parity), #4 (live-network-verified
non-equity provider proof), and #5 (explicit operator enablement decision).
```

**Recommended next patch:** `REGISTRY-V2-GATE-PARITY-01`.
