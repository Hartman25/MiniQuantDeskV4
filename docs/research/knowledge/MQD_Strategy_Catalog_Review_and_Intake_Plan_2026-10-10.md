# MQD — Strategy Idea Catalog Review and Safe AI Intake Plan

**Reviewed:** 2026-10-10 · **Purpose:** Inform `V4-STRATEGY-FACTORY-RESEARCH-BACKTEST-FULL-COMPLETION-01` without changing its frozen economic or operator authority.
**Basis:** Direct inspection of six uploaded XLSX workbooks and the corresponding Reddit CSV. This is a **file-content review**, **not** independent verification of their external URLs, paper findings, or trading profitability, and **not** a comparison against a live MQD Research registry.

## 1. Executive assessment

The catalogs are useful, unusually careful research-intake material. They combine ordinary technical-strategy ideas with hypothesis design, market mechanisms, alternative explanations, negative controls and data-provenance cautions. The principal engineering value is **idea coverage plus better falsification**, not a new batch of independently proved strategies.

**Raw total: 456 idea/proposal rows across six workbooks.** This is **not 456 unique/economically independent strategy trials**, and not all rows are trade ideas. Many are diagnostic studies, controls, and follow-up questions. *No* workbook's status field establishes MQD OOS alpha or Promotion authority. Semantic novelty, feasibility and execution compatibility require later repository/DB checks.

## 2. Verified workbook-level inventory

| Workbook / sheet | Main rows | Supporting evidence in file | Recommendation |
|---|---:|---|---|
| `MQD_Reddit_Trading_Strategy_Idea_Catalog_2026-10-09.xlsx` / `ALL_147_IDEAS` | 147 | `SOURCES_64`: 64 source rows/unique linked discussion URLs; 21 subreddit communities shown in start sheet; `M1_EQUITY_ETF`: 95 rows; `DEFERRED_ASSETS`: 52 | Excellent bulk **idea intake**, but many variants and forum-derived assumptions; reference source/provenance and run novel-idea gate |
| `MQD_Academic_Official_Strategy_Idea_Catalog_2026-10-09.xlsx` / `IDEAS` | 121 | `SOURCES`: 57 rows; `M1 SCREENS`: 73 rows; `LATER ASSETS`: 48; explicit source claim vs proposed falsifiable question, negative control and risk | Best foundation for **evidence-aware normalization**; never confuse paper finding with MQD executable strategy |
| `MQD_Deep_Mechanisms_Investor_Behavior_News_2026-10-09.xlsx` / `Research Hypotheses` | 36 | 12 mechanism analysis rows, 29 primary source rows, 14 research guardrails | Prefer as **research-design and mechanism library**; many rows should remain diagnostic |
| `MQD_Investor_Psychology_Order_Flow_Price_Volume_2026-10-09.xlsx` / `Mechanism Probes` | 26 | 12 M1 first-look rows, 24 primary source rows, 26 overlap candidates, 12 non-negotiable controls | Especially useful for avoiding false attribution from price/volume data |
| `MQD_Deep_Dive_Shocks_Attention_Institutions_2026-10-09.xlsx` / `All Proposals` | 86 | 22 M1 diagnostics, 53 sources, 11 decision-control rows | Well-designed rival-explanation and negative-control supply; 10 guardrail rows are not separate economic strategies |
| `MQD_Four_Area_Deep_Dive_Edge_Research_2026-10-09.xlsx` / `Four Research Areas` | 40 | Four 10-row families (price-volume, overnight/intraday, news-novelty, institutional-demand); 15 M1 triage, 24 sources, 48 overlap hints | Practical, bounded cross-file dedup and falsification starting point; 40 are **FAD** IDs, not four duplicate IDs |
| **Total** | **456** | Source counts are workbook-local and overlap; never sum into a claim of unique papers | Build one canonical intake registry and preserve source-by-source lineage |

**CSV variant:** `MQD_Reddit_Trading_Strategy_Idea_Catalog_2026-10-09.csv` carries 147 unique `id` rows corresponding to the 147-idea workbook. Its column names use `snake_case` (`id`, `tier`, `family`, `name`, …), whereas the XLSX uses display names (`ID`, `Complexity`, `Family`, `Idea / hypothesis`, …). They are **two file representations of one population**, not 294 strategy ideas. Exact normalized row-equivalence still merits an import test; the matching row counts are not sufficient by themselves.

**Duplicate attachments:** The `(1)` copy of the Reddit XLSX and its original are byte-identical by SHA-256; likewise the two Reddit CSV copies and two academic XLSX copies. Do not register duplicates because multiple file copies exist.

**Additional historical input not present in this six-workbook set:** The separately discussed 200-row `EXT-001...EXT-200` external strategy catalog is **not among the six reviewed XLSX files**. Its existing canonical MQD intake history, if available, must be consulted during dedup; it cannot be assumed present in this attachment set.

## 3. Quality strengths

1. **Clear separation of sources from test hypotheses.** The academic file has distinct `Actual Source Finding`, `Source Limitation`, `Falsifiable Hypothesis`, `Connection to Paper` and `Negative Control` fields. The deeper files explicitly identify when proposals are *derived* from primary research rather than direct source-reproduced strategies.
2. **Fail-closed maturity labeling.** Reddit `Proof status` says `IDEA ONLY / NOT TESTED IN MQD` for all 147 rows. Academic `Authority Status` says `IDEA ONLY — no MQD test, novelty decision, or promotion` for all 121, and its `Dedup Status` explicitly says semantic dedup pending. Other files similarly retain unverified/not-run statuses.
3. **Mechanism taxonomy.** Several files explicitly distinguish information incorporation, transient liquidity pressure, and risk compensation—far more informative than assuming a price pattern has one cause.
4. **Point-in-time discipline.** Event-time availability, earnings/filing release clocks, index changes, historical membership and vendor-latency problems are visible instead of hidden.
5. **Competitor explanations and nulls.** Volume-conditioned reversal, residual shocks, and institutional flow hypotheses propose matching, negative controls and lead/lag tests rather than assuming favorable results.
6. **M1/later triage already exists.** Source-authored M1 screens can prioritize inspection, although they are not themselves canonical MQD admission authority.

## 4. Material caveats before implementation

- **Cross-catalog semantic duplicates are still unresolved.** Similar titles and slight parameter variants must not become separate independent discoveries. The supplied overlap-hint sheets explicitly disclaim novelty proof.
- **Not every M1 screen is immediately executable.** Reddit has 95 rows in an M1-oriented sheet, but only 55 of all 147 are marked literally `M1 screen`; other M1-related rows demand external data, multi-symbol logic, or represent benchmarks. Academic 73 M1 screens similarly contain conditional research and validation support, not 73 approved executable strategy declarations.
- **Source existence is not source verification.** The review confirms URL fields are populated, not that every URL resolves, that linked material exactly supports the idea, or that the claims replicate. The academic source `Verification State` and limitations should be preserved and independently checked only when load-bearing.
- **Daily OHLCV is not signed institutional/retail flow.** The psychology book explicitly warns this; OBV/CLV/dollar volume are only proxies. Real imbalance, auction, borrow and quote features need separate point-in-time data.
- **More sophisticated ideas often require expensive or unavailable inputs.** PIT earnings-consensus, full options IV surfaces, intraday depth, dated index flows, news archives, historical delisted membership and borrow records are not obtainable from ordinary adjusted daily bars.
- **Research controls must not be counted as candidate hypotheses.** Examples: risk-matched benchmarks, placebo tests, held-out validation, chronology controls and tail-risk attribution are functions of the testing system, not additional alpha strategies.
- **Source IDs differ by workbook.** Use composite `source_catalog_id + item_id + source_file_hash`, never a bare `P01`/short ID (several sheets independently use `P01`); support both XLSX display names and CSV normalized names.
- **No URL, academic idea, LLM decision, or indicator definition confers trading authority.** Explicit operator-approved economic policy and deterministic native implementation remain required.

## 5. High-value initial *diagnostic* clusters, not alpha claims

| Cluster | Examples found | Suitable first action | Why |
|---|---|---|---|
| Trend, SMA, breakout and prior-high patterns | `RDI-004` 200-day trend, `RDI-005` buffered gate, `RDI-008` prior channel breakout, `ACI-001` trend continuation | **Dedup against existing Rust native engines / previous campaigns first** | Likely overlap with native trend, SMA, absolute momentum and breakout families; rerunning renamed ideas adds little independent evidence |
| Short-run mean reversion | `RDI-009` RSI2 pullback, `ACI-014` short-horizon reversal | Identify precise entry/exit/sizing and earlier rejected tests | Signals can lose after costs; “RSI2” is not the entire Connors RSI definition |
| Volume-conditioned shock paths | `BNS-001`, `IPO-001`, `DDI-001`, `FAD-PV-001` | **Merge into one diagnostic family**, compare market-residual shock with risk- and volatility-matched controls | These ask related continuation-versus-reversal questions; volume is an imperfect liquidity proxy |
| Overnight vs intraday pressure | `IPO-005`, `FAD` overnight/intraday family | Test accounting and temporal decomposition before strategy generation | Opens, closes, gap adjustment, market sessions and executable order time cause spurious results if not pinned |
| Fresh versus repeated news | `BNS-029`, `IPO-011`, DDI news-novelty group | Define text version, first-source timestamp and duplicate-story controls; defer economics until PIT archive available | AI is suited to extraction/novelty, but later duplicated articles and vendor availability can create severe leakage |
| Institutional forced demand | `DDI-055` index deletion/addition asymmetry and FAD institutional family | Check actual PIT index announcement/effective dates, holdings/flow data | Index and fund flow cannot be inferred cleanly from daily prices alone |
| Real auction imbalance | `DDI-064` signed closing auction imbalance | `REQUIRES_UNAVAILABLE_DATA` pending trustworthy PIT source | Requires observed auction feed; unsuitable for daily-only M1 backtest |
| Tail risk and benchmark matching | `BNS-021` left-tail attribution, other risk-match controls | Add as **non-strategy validation requirements**, not candidate trials | Positive gross Sharpe may compensate risk rather than represent independent alpha |

**None of these examples is a recommendation to initiate a new economic campaign.** Current M1 search-population and holdout/operator restrictions continue to apply.

## 6. Recommended single authority for Factory intake

Input ingestion should preserve **two layers**:

**Raw source row:** source workbook SHA-256, sheet, row index, original row bytes/strings, URL, source type, authors/publisher if provided, metadata and licensing constraints. Keep immutable and never execute code or workbook formulas as instructions.

**Normalized `StrategyIdea` proposal:** stable composite source references; explicit/inferred/unknown fields; machine-readable economic hypothesis; possible family and mechanism; *rival explanations*; feature/data dependencies; entry/exit/sizing unknowns; temporal availability; potential semantic neighbors; *provisional* disposition; AI provider/model/version and raw structured proposal hash.

Canonical admission and any trial identity must be decided by deterministic MQD code. LLM “confidence” is a triage hint, not a trading or research authorization credential.

Suggested importer stages:

1. XLSX/CSV safely parse and hash. Match aliases in a version-pinned import mapping; preserve unmatched columns instead of discarding them.
2. Produce one immutable source-row record with composite identity; detect byte-identical duplicate copies of a workbook before repeating intake.
3. Label proposal kinds: `STRATEGY_HYPOTHESIS`, `MECHANISM_DIAGNOSTIC`, `RESEARCH_CONTROL`, `BENCHMARK`, `INSUFFICIENT_RULES`, or `FUTURE_ASSET`. Avoid assuming every row merits an executable trial.
4. Retrieve definitions from the separate trading knowledge base; invoke local AI *optionally* for extraction/semantic-neighbor suggestions.
5. Verify source-span grounding. Require unknowns and source-vs-inference tagging; reject unsupported numeric parameters as source facts.
6. Compare semantic identity with canonical MQD registry/history/EXT catalog; preserve relation class, evidence and unresolved matches.
7. Gate dataset, PIT universe, source authorization, short feasibility, native strategy support, calendar, costs and policy.
8. Only under separately authorized predeclaration, register whole unique comparable trial population before economic results and call the native Rust strategy/backtest path.
9. Make rejected, deferred, unsupported, not-evaluable and failed attempts durable and visible. No winner-only registration or retroactive holdout clearance.

## 7. Suggested acceptance proof

- **Intake cardinality:** exact six workbook counts 147/121/36/26/86/40 before dedup; independent CSV count=147; repeated copies do not double records. Note: support all rows but **do not** auto-register all as economic trials.
- **Source fidelity:** one test per source-specific header dialect; original text preserved even when model paraphrases; ambiguous source ID `P01` resolves through workbook-scoped identity.
- **Grounded extraction:** known explicit rule can be recovered; unspecified RSI window, threshold, trading size and order clock remain `UNKNOWN`.
- **Classification:** risk control and diagnostic do not become tradable candidates; options/auction/quotes missing PIT source refuse correctly.
- **Dedup:** trend/price-volume semantic neighbors are surfaced without inventing identity equivalence; distinct economically material rules receive distinct identity when admitted.
- **Security:** spreadsheet formula/macros/external links and hostile web text cannot execute instructions, access credentials or mutate Research/Broker/Promotion.
- **Restart:** rerun exact same intake record produces idempotent result; corrupted/missing artifact fails closed; AI provider outage falls back to deterministic `NEEDS_FORMALIZATION`.
- **Read-only boundaries:** no catalog import calls historical data providers, registers a Research trial, consumes a holdout, promotes a strategy, or submits Paper/Live orders unless separately authorized through the existing production seam.

## 8. Prioritization and independent-review verdict

**CONFIRMED (file-inspection basis):** Structured source/proposal/limitation/control fields exist; raw row counts above are accurate; all Reddit and Academic idea statuses are explicitly untested; the six book populations require cross-catalog semantic dedup; source-URL coverage is present.

**LIKELY:** Large economic overlap with prior MQD native trend/momentum/reversal work and with other catalogs; much of the new unique value may lie in null controls, mechanism probes and source-aware feature specifications rather than entirely new profitable strategy families.

**UNKNOWN — REQUIRES PROOF:** Exact semantic novelty after querying MQD Git/DB; external source link validity and fidelity; historical data entitlements/PIT coverage; native implementability of every idea; availability of real news/order book/options data; economics, OOS credibility or profitability. No conclusion of promising alpha is warranted from an idea catalog.

**Recommended project decision:** Include **all six** in Strategy Factory intake and reference retrieval. Do **not** run all 456 as backtests automatically. Normalize, classify, cluster, dedup and gate first. Pair the catalogs with the companion glossary `MQD_Trading_Indicators_Signals_Market_Knowledge_v1.md` / `.json` to help the local AI understand terms while preserving source-typed uncertainties and deterministic MQD authority.

**Safe Git placement after review:** `docs/research/knowledge/` for the two Markdown files and JSON companion. Share files with the currently running Claude controller rather than launching a parallel Strategy Factory or changing shared code while it runs.

## Appendix: verified input-file SHA-256 hashes

| Input file | SHA-256 |
|---|---|
| `MQD_Reddit_Trading_Strategy_Idea_Catalog_2026-10-09.xlsx` | `0f9bd5141615611755fdaabaa5a36ef525632f99981b478f23332b38bacfe320` |
| `MQD_Reddit_Trading_Strategy_Idea_Catalog_2026-10-09.csv` | `6e00f9a1cda95baf3385aab9cd49a73e8c23e134f08c251790c0f880a9ac0b12` |
| `MQD_Academic_Official_Strategy_Idea_Catalog_2026-10-09.xlsx` | `57f341b1517d57a0130dd11bfa5b617b8e153f327f69fd9da017f9db6ff9d891` |
| `MQD_Deep_Mechanisms_Investor_Behavior_News_2026-10-09.xlsx` | `a3e52c29b0f543c842c14bcd591a9bc7b3c033e91e7e100e871f16acfbcfbc78` |
| `MQD_Investor_Psychology_Order_Flow_Price_Volume_2026-10-09.xlsx` | `d1519702c2d31c487fd5b40840dc619cf155a2736b82df3b57cad43a38e89ced` |
| `MQD_Deep_Dive_Shocks_Attention_Institutions_2026-10-09.xlsx` | `5b4632719589757a1fe08441a1b11853f03452931817cd766967b04a9e17491b` |
| `MQD_Four_Area_Deep_Dive_Edge_Research_2026-10-09.xlsx` | `e0926318100a8ed9c7429769e2409e539dd5214f574b13aadae71ee7070b3415` |
