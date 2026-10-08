"""Reviewer-authored semantic classification of the 200 external catalog rows (data only).

Each line: ID | blockers | novelty | direction | mechanism | relations | missing | reason.
Everything else (feasibility, primary disposition, population membership) is DERIVED in disposition.py.
No result, return claim, catalog priority or readiness score is an input. Text is untrusted data throughout.

blockers (any order; '-' = none): F future asset class (futures/options/FX/crypto/non-US/cross-asset)
  L ML or alternative-data framework (models, news/sentiment)   D additional authoritative data
  P multi-symbol portfolio engine (ranks, rotation, pairs)      S short/borrow/hedge is native to the rule
  X new execution policy (same-bar close/open orders, resting grids)   U rule not computable from the text
novelty: EXACT PARAM MIRROR COMPL COMP SEM NEW UNK (see the rulebook in the consolidated record).
relations: kind:target joined by ';' with kind in dup param sem adj comp mirror compo pair neighbor.
"""

ALIASES = {
    "TSMA": "trend_sma50", "DSMA": "dual_sma_50_200_trend", "PMR": "pullback_mean_reversion_20_2",
    "AM252": "absolute_momentum_252", "NH252": "near_high_momentum_252_3pct",
    "TP5": "trend_pullback_5d_4pct_hold5", "TOM": "turn_of_month_last1_first3", "HAL": "halloween_nov_apr",
    "TRB50": "trading_range_breakout_50d_hold10",
    "F01": "monthly_multihorizon_abs_momentum_consensus_v1", "F02": "trend_filtered_rsi5_reversion_v1",
    "F03": "trend_filtered_extreme_3d_atr_reversal_v1", "F04": "close_channel_100_50_trend_v1",
    "F05": "monthly_10month_trend_timing_v1", "F06": "trend_filtered_zscore20_reversion_v1",
    "F07": "volatility_contraction_breakout_v1", "F08": "monthly_12_minus_1_abs_momentum_v1",
    "F09": "delayed_overnight_gap_reversal_v1", "F10": "monthly_52week_high_proximity_v1",
}

TABLE = """
EXT-001|LS|NEW|LONG_SHORT|ML_DIRECTIONAL|neighbor:SHORT_WAVE_02|lookback;features;probability threshold|ML classifier on sector constituents; no ML framework or PIT constituent authority
EXT-002|LD|NEW|LONG_SHORT|NEWS_SENTIMENT|-|news source and timestamps;sentiment threshold;holding period|timestamped news plus intraday prices are alternative data
EXT-003|FL|NEW|LONG_SHORT|ML_FORECAST|-|wavelet and SVM settings;forecast threshold|FX instrument and ML model
EXT-004|LDS|NEW|LONG_SHORT|ML_DIRECTIONAL|-|features;tree settings;threshold|ML on intraday bars
EXT-005|DPS|NEW|LONG_ONLY|FUNDAMENTAL_QUALITY|-|G-Score components;rank cutoffs|point-in-time fundamentals and cross-sectional ranking
EXT-006|PS|NEW|MARKET_NEUTRAL|PAIRS_OU|-|spread model;entry boundaries;hedge ratio|two-leg pair rule
EXT-007|PSD|NEW|MARKET_NEUTRAL|PAIRS_INTRADAY_ETF|neighbor:EXT-047|pair choice;divergence threshold|intraday two-leg relative value
EXT-008|U|SEM|LONG_SHORT|TREND_ICHIMOKU|sem:S03;sem:S04;adj:DSMA|Tenkan/Kijun/Senkou periods;signal choice;timeframe;stock universe|price-only trend state; periods not stated
EXT-009|LS|NEW|LONG_SHORT|ML_FORECAST|-|window length;network;threshold|neural forecaster
EXT-010|U|PARAM|LONG_FLAT|TREND_PRICE_VS_SMA|param:TSMA;param:S02|SMA length;leveraged ETF list;rebalance cadence|price-vs-SMA gate; only the instrument class differs from the rejected rule
EXT-011|DP|NEW|LONG_SHORT|INTRADAY_OPEN_MOMENTUM|-|opening window;return threshold or rank|intraday opening-return ranking
EXT-012|PS|SEM|LONG_SHORT|CS_MOMENTUM_NEARHIGH|adj:AM252;adj:NH252;adj:S01;adj:S10|ROC window;nearness metric;rank cutoff|cross-sectional momentum plus high proximity
EXT-013|DPS|SEM|LONG_SHORT|CS_RESIDUAL_MOMENTUM|adj:AM252;adj:S01|factor model;residual lookback|needs factor returns; momentum mechanism
EXT-014|F|SEM|LONG_SHORT|TS_MOMENTUM_FUT|adj:S01;adj:AM252|trend lookback;vol scaling|futures time-series trend
EXT-015|DPS|NEW|LONG_SHORT|PRICE_EARNINGS_MOMENTUM|neighbor:S01|price lookback;earnings-growth metric;rank|needs point-in-time fundamentals
EXT-016|F|SEM|LONG_SHORT|TS_MOMENTUM_FUT|adj:S01|weights;vol estimator;leverage cap|futures momentum
EXT-017|DPS|NEW|LONG_ONLY|IDIO_SKEWNESS|-|expected-skewness model;cutoff|factor model and fundamentals-grade data
EXT-018|FD|NEW|LONG_SHORT|FX_TAIL_RISK|-|tail-risk metric;rank|FX spot/forward/rates
EXT-019|DPS|NEW|LONG_ONLY|EARNINGS_SURPRISE|-|SUE definition;rank|point-in-time earnings
EXT-020|PSU|NEW|LONG_SHORT|CS_SAME_MONTH_SEASONALITY|neighbor:S14|history years;rank buckets|cross-sectional seasonal rank; history length unstated
EXT-021|PS|NEW|MARKET_NEUTRAL|STATARB_RESIDUAL_MR|neighbor:S06|residual model;z-score;hedge ratios|basket/pair mean reversion
EXT-022|DPS|NEW|LONG_SHORT|MULTI_FACTOR_FUNDAMENTAL|-|factor definitions;ranks|FF5 characteristics need fundamentals
EXT-023|DPU|UNK|LONG_ROTATION|SIZE_ROTATION_SEASONAL|-|signal rule;market-cap breakpoint|the catalog itself flags a source-page mismatch; open the article first
EXT-024|D|NEW|LONG_SHORT|VIX_PERCENTILE_TIMING|-|percentile thresholds;exit rule|requires a VIX series; no VIX data authority exists
EXT-025|DPS|NEW|LONG_ONLY|VALUE_PE|-|P/E definition;rank|fundamentals
EXT-026|PS|SEM|LONG_SHORT|CS_LOW_BETA|adj:DISCOVERY_01|beta lookback;benchmark;weights|cross-sectional low-risk anomaly, adjacent to the rejected low-volatility wave
EXT-027|DP|NEW|LONG_ONLY|CAPE_VALUE|-|CAPE source;terciles|valuation data not available
EXT-028|PS|SEM|LONG_SHORT|CS_MOMENTUM|adj:AM252;adj:S01|12m return;top/bottom 10|cross-sectional 12-month momentum
EXT-029|PS|NEW|LONG_SHORT|CS_12M_CYCLE_SEASONAL|neighbor:S14|January-return rank;deciles|cross-sectional seasonal rank
EXT-030|PS|SEM|LONG_SHORT|CS_LOW_BETA|adj:DISCOVERY_01|beta lookback;benchmark;weights|low-beta anomaly
EXT-031|FS|NEW|LONG_SHORT|VIX_FUTURES_CARRY|-|roll metric;hedge ratio|VIX futures and ES hedge
EXT-032|-|NEW|LONG_FLAT|CAL_PRE_HOLIDAY|neighbor:TOM;neighbor:HAL;neighbor:S14|-|long the two sessions before each exchange holiday; window length and holiday source are stated; session-calendar convention fixed at predeclaration
EXT-033|FP|NEW|LONG_SHORT|CS_REVERSAL_FUT|-|volume and open-interest screens|futures cross-section
EXT-034|PS|SEM|LONG_SHORT|CS_MOMENTUM_VOLUME|adj:AM252;adj:S12|momentum rank;volume rank|momentum with volume rank
EXT-035|D|NEW|LONG_SHORT|LUNAR_CYCLE|-|lunar calendar authority|requires an astronomical calendar authority that MQD does not hold
EXT-036|DPS|NEW|LONG_SHORT|QUALITY_ROA|-|ROA definition;size buckets|fundamentals
EXT-037|U|NEW|LONG_FLAT|CAL_JANUARY_BAROMETER|neighbor:HAL;neighbor:S01|January-return measurement window;holding start and end;cash proxy|annual state from one calendar month; definitions not stated
EXT-038|PS|SEM|LONG_SHORT|CS_MOMENTUM_REVERSAL_VOL|adj:AM252|vol rank;performance rank|cross-sectional momentum within a volatility group
EXT-039|PS|NEW|MARKET_NEUTRAL|PAIRS_COPULA|-|copula family;z-score|pairs
EXT-040|DPS|NEW|LONG_SHORT|EARNINGS_QUALITY|-|quality components;weights|fundamentals
EXT-041|FU|UNK|UNKNOWN|MANUAL_REVIEW|-|signal not stated|the catalog itself flags a source-page mismatch; no rule is stated
EXT-042|FS|NEW|MARKET_NEUTRAL|COMMODITY_SPREAD_MR|-|hedge ratio|WTI-Brent futures spread
EXT-043|PS|SEM|LONG_ONLY|CS_MOMENTUM|adj:AM252|momentum lookback;terciles|REIT cross-sectional momentum
EXT-044|U|NEW|LONG_FLAT|CAL_OPEX_WEEK|neighbor:TOM;neighbor:S14|expiration-week definition;window start and end day;holiday shift|options-expiration calendar is not an MQD authority and the week is not defined
EXT-045|PS|SEM|LONG_SHORT|CS_ROTATION_MOMENTUM|adj:AM252;adj:F08;adj:S01|12m total return;top-1 selection|style-ETF rotation needs a multi-symbol engine
EXT-046|PS|COMP|LONG_SHORT|CS_MOMENTUM_MARKET_STATE|compo:AM252;adj:S01|6m momentum;12m index return|cross-sectional momentum gated by an absolute-momentum market state
EXT-047|PS|NEW|MARKET_NEUTRAL|PAIRS_COUNTRY_ETF|-|pair selection;divergence threshold|pairs
EXT-048|DPS|NEW|LONG_SHORT|ASSET_GROWTH|-|annual asset change;rank|fundamentals
EXT-049|DPS|NEW|LONG_SHORT|ACCRUAL|-|accrual definition;deciles|fundamentals
EXT-050|DPS|NEW|LONG_SHORT|SENTIMENT_STYLE_ROTATION|-|sentiment measure;style definitions|sentiment series
EXT-051|-|EXACT|LONG_FLAT|CAL_TURN_OF_MONTH|dup:TOM;param:S14|-|buy the session before month-end, exit on the third session of the new month: last 1 plus first 3 sessions, the registered turn_of_month_last1_first3 on SPY
EXT-052|PS|SEM|LONG_SHORT|CS_MOMENTUM_REVERSAL|adj:AM252|winner/loser groups;short-term change|cross-sectional momentum-reversal hybrid
EXT-053|P|SEM|LONG_ROTATION|REL_MOMENTUM_ROTATION|adj:AM252;adj:F01|lookback;pair definitions|two-asset relative momentum
EXT-054|FD|SEM|LONG_SHORT|FUT_MOM_CARRY|adj:S01|1m momentum;roll return|futures curve
EXT-055|D|NEW|LONG_FLAT|MACRO_FED_MODEL_GOLD|-|earnings-yield source;bond-yield source|requires equity earnings yield and bond yield series
EXT-056|DPS|NEW|LONG_ONLY|VALUE_BM|-|book value;quintiles|fundamentals
EXT-057|FP|NEW|LONG_SHORT|COMMODITY_CARRY|-|roll-return rank|futures curves
EXT-058|X|NEW|LONG_FLAT|OVERNIGHT_DRIFT|neighbor:F09;neighbor:S08|close/open definition|buys the same bar's close and sells the next open; same-bar close fills are not an accepted execution policy
EXT-059|DPS|NEW|LONG_ONLY|SIZE_PREMIUM|-|market-cap ranking|point-in-time market cap
EXT-060|FP|SEM|LONG_SHORT|CS_MOMENTUM_FUT|adj:AM252|lookback;ranks|futures cross-section
EXT-061|FD|NEW|SHORT_VOL|OPT_SHORT_STRADDLE_HEDGED|-|strikes;hedge|options
EXT-062|DPS|SEM|LONG_SHORT|CS_LIQUIDITY|adj:WAVE06_LIQ01|turnover metric;size quartile|liquidity factor, adjacent to the rejected Amihud wave
EXT-063|PS|NEW|LONG_SHORT|CS_LONG_HORIZON_REVERSAL|-|36m return;ranks|cross-sectional country ETFs
EXT-064|PS|SEM|LONG_ROTATION|CS_MOMENTUM_COUNTRY|adj:AM252;adj:S01|12m momentum|country-ETF rotation
EXT-065|DPS|SEM|LONG_ONLY|CS_MOMENTUM|adj:AM252|12m return;universe|large-cap stock momentum
EXT-066|FD|NEW|LONG_SHORT|FX_CARRY|-|policy rates|FX
EXT-067|PS|NEW|MARKET_NEUTRAL|PAIRS_SSD|-|formation window;SSD threshold|pairs
EXT-068|P|SEM|LONG_ROTATION|CS_ROTATION_MOMENTUM|adj:AM252;adj:F01|lookback;number selected|asset-class ETF rotation
EXT-069|PS|SEM|LONG_ONLY|CS_LOW_VOL|adj:DISCOVERY_01|1y volatility;rank|low-volatility anomaly, already rejected as a wave
EXT-070|P|SEM|LONG_ROTATION|CS_ROTATION_MOMENTUM|adj:F01;adj:F08;adj:AM252|lookback;top N|sector rotation needs a multi-symbol engine
EXT-071|U|PARAM|LONG_FLAT|TREND_MONTHLY_10M_SMA|param:F05;adj:TSMA|window inclusion of the current month-end;asset-class ETF list|per-asset month-end close versus a 10-month average: the registered monthly_10month_trend_timing_v1 differing only in window convention
EXT-072|PS|SEM|LONG_SHORT|CS_REVERSAL|adj:S05;adj:S11|1m return ranks|cross-sectional reversal
EXT-073|F|SEM|LONG_SHORT|FX_MOMENTUM|adj:S01|12m FX return|FX
EXT-074|FU|SEM|LONG_SHORT|FX_MOMENTUM_FILTERED|adj:S01|filter method;cutoff;window|FX
EXT-075|FU|SEM|LONG_SHORT|BREAKOUT_DYNAMIC|adj:S04;adj:F04|range lookback;multipliers|multi-asset including futures and FX
EXT-076|DS|NEW|LONG_SHORT|MACRO_OIL_PREDICT|-|regression window;predictors|crude and T-bill series
EXT-077|FDU|NEW|LONG_SHORT|INTRADAY_DUAL_THRUST|-|range lookback;K1;K2|intraday opening-range style
EXT-078|DPSU|NEW|LONG_ONLY|FUNDAMENTAL_SELECTION|-|fields and thresholds|fundamentals
EXT-079|DPS|NEW|LONG_SHORT|FUNDAMENTAL_FACTOR|-|factor set;weights|fundamentals
EXT-080|FU|SEM|LONG_SHORT|FX_MR_MOMENTUM|adj:S01;adj:S06|windows;regime combiner|FX hybrid
EXT-081|DPS|NEW|LONG_SHORT|CAPM_ALPHA|-|estimation window;benchmark|risk-free series and ranks
EXT-082|DPS|NEW|MARKET_NEUTRAL|PAIRS_INTRADAY|-|rolling windows;z-score|intraday pairs
EXT-083|U|SEM|LONG_SHORT|REVERSAL_PRICE_ACTION|adj:S05;adj:S06;adj:S11;adj:F02;adj:F06|return window;thresholds;exit rule|short-horizon reversal; no numeric rule
EXT-084|U|SEM|LONG_SHORT|TREND_AROON|adj:S03;adj:S04;adj:F04|Aroon lookback;crossover definition|range-extreme trend state
EXT-085|DU|SEM|LONG_SHORT|TREND_AROON|adj:S03;adj:S04;pair:EXT-084|Aroon lookback|intraday timeframe variant of EXT-084
EXT-086|U|SEM|LONG_SHORT|REVERSION_BOLLINGER|adj:PMR;adj:S06;adj:F06|band length;band width;re-entry definition;exit rule|band re-entry reversion
EXT-087|DU|SEM|LONG_SHORT|REVERSION_BOLLINGER|adj:PMR;adj:S06;pair:EXT-086|band length;band width|intraday timeframe variant of EXT-086
EXT-088|DU|SEM|LONG_SHORT|REVERSION_BB_CONNORS_RSI|adj:PMR;adj:S05;adj:F02|band settings;Connors RSI thresholds|intraday
EXT-089|DU|SEM|LONG_SHORT|TREND_CHANDELIER_EMA|adj:S03|EMA;ATR;multiplier|intraday
EXT-090|DU|SEM|LONG_SHORT|TREND_EMA_ADX_HA|adj:S03|EMA;ADX threshold|intraday
EXT-091|U|SEM|LONG_SHORT|TREND_EMA_CROSS|adj:S03;adj:DSMA|fast and slow EMA periods|exponential instead of simple averages
EXT-092|DU|SEM|LONG_SHORT|TREND_EMA_CROSS|adj:S03;pair:EXT-091|fast and slow EMA periods|intraday timeframe variant of EXT-091
EXT-093|U|COMPL|LONG_SHORT|CONTRARIAN_EMA_CROSS|comp:EXT-091;adj:S03;adj:SH03|fast and slow EMA periods|long when the fast EMA is below the slow: the complement of EXT-091 long state
EXT-094|DU|COMPL|LONG_SHORT|CONTRARIAN_EMA_CROSS|comp:EXT-092;pair:EXT-093;adj:S03|fast and slow EMA periods|intraday timeframe variant of EXT-093
EXT-095|U|SEM|LONG_SHORT|TREND_MACD_CROSS|adj:S03;adj:DSMA|MACD fast/slow/signal periods|moving-average difference crossover
EXT-096|DU|SEM|LONG_SHORT|TREND_MACD_CROSS|adj:S03;pair:EXT-095|MACD periods|intraday timeframe variant of EXT-095
EXT-097|U|EXACT|LONG_SHORT|REVERSION_BOLLINGER|dup:EXT-086|band length;band width|normalized rule text identical to EXT-086 at the same timeframe; the catalog cannot distinguish them
EXT-098|DU|EXACT|LONG_SHORT|REVERSION_BOLLINGER|dup:EXT-087|band length;band width|normalized rule text identical to EXT-087 at the same timeframe
EXT-099|FD|NEW|BEARISH_OPTIONS|OPT_BEAR_CALL_LADDER|-|leg quantities;strikes|options
EXT-100|FD|NEW|BEARISH_OPTIONS|OPT_BEAR_PUT_LADDER|-|leg quantities;strikes|options
EXT-101|FD|NEW|BEARISH_OPTIONS|OPT_BEAR_PUT_SPREAD|-|strikes;target;stop|options
EXT-102|FD|NEW|BULLISH_OPTIONS|OPT_BULL_CALL_LADDER|-|leg quantities;strikes|options; normalized text collides with EXT-105 but the source structures differ by name, so not merged
EXT-103|FD|NEW|BULLISH_OPTIONS|OPT_CALL_RATIO_BACKSPREAD|-|ratio;strikes;risk limits|options
EXT-104|FD|NEW|BULLISH_OPTIONS|OPT_BULL_CALL_SPREAD|-|strikes;target;stop|options
EXT-105|FD|NEW|BULLISH_OPTIONS|OPT_BULL_PUT_LADDER|-|leg quantities;strikes|options; normalized text collides with EXT-102 but the source structures differ by name, so not merged
EXT-106|FD|NEW|LONG_OPTIONS|OPT_BUY_EMA_CROSS|neighbor:EXT-091|EMA periods;strike selection|options on an EMA signal
EXT-107|FD|NEW|LONG_OPTIONS|OPT_BUY_MULTI_CANDLE|-|candle count;strike distance|options
EXT-108|FD|NEW|LONG_OPTIONS|OPT_BUY_RSI_MACD|neighbor:EXT-126|RSI and MACD settings|options on an RSI-MACD signal
EXT-109|FD|NEW|LONG_OPTIONS|OPT_BUY_RSI_PSAR|-|RSI and PSAR settings|options
EXT-110|FD|NEW|LONG_OPTIONS|OPT_DELTA_TARGET|-|target delta;target;stop|options
EXT-111|FD|NEW|BULLISH_OPTIONS|OPT_CALL_DIAGONAL|-|expiries;strikes;trail|options
EXT-112|FD|NEW|LONG_VOL|OPT_LONG_IRON_BUTTERFLY|-|strikes;expiry|options
EXT-113|FD|NEW|SHORT_VOL|OPT_IRON_CONDOR|-|wing widths;re-entry|options
EXT-114|FD|NEW|LONG_SHORT|OPT_PUT_CALL_RATIO|-|window;thresholds|options sentiment
EXT-115|FD|NEW|SHORT_VOL|OPT_SHORT_CALL_BUTTERFLY|-|strikes;exits|options
EXT-116|FD|NEW|SHORT_VOL|OPT_SHORT_IRON_BUTTERFLY|-|wing width;re-entry|options
EXT-117|FD|NEW|BULLISH_OPTIONS|OPT_JADE_LIZARD|-|strikes;net-value exits|options
EXT-118|FD|NEW|SHORT_VOL|OPT_SHORT_STRADDLE|-|strike;stop;target|options
EXT-119|FD|NEW|SHORT_VOL|OPT_SHORT_STRADDLE|pair:EXT-118|strike;stop;target|options; same rule text as EXT-118 at a different timeframe
EXT-120|FD|NEW|SHORT_VOL|OPT_SHORT_STRANGLE|-|OTM distance;risk limits|options
EXT-121|FD|NEW|LONG_VOL|OPT_STRADDLE|-|leg directions;strike|options
EXT-122|FD|NEW|LONG_VOL|OPT_STRANGLE|-|leg directions;strikes|options
EXT-123|DU|SEM|LONG_SHORT|VOLUME_PRICE_POCKET_PIVOT|adj:S12;adj:WAVE06_VOL01|pivot,volume and EMA settings|intraday volume-price setup
EXT-124|U|SEM|LONG_SHORT|CONTRARIAN_RSI|adj:S05;adj:F02|RSI length;thresholds|RSI condition not stated
EXT-125|DU|SEM|LONG_SHORT|CONTRARIAN_RSI|adj:S05;pair:EXT-124|RSI length;thresholds|intraday timeframe variant of EXT-124
EXT-126|U|SEM|LONG_SHORT|HYBRID_RSI_MACD|adj:S05;adj:S03|RSI thresholds;MACD periods|RSI recovery plus MACD cross
EXT-127|DU|SEM|LONG_SHORT|HYBRID_RSI_MACD|adj:S05;adj:S03;pair:EXT-126|RSI thresholds;MACD periods|intraday timeframe variant of EXT-126
EXT-128|U|SEM|LONG_SHORT|OSC_STOCHASTIC_CROSS|adj:S03;adj:S10|%K and %D lengths|range-position oscillator crossover
EXT-129|DU|SEM|LONG_SHORT|OSC_STOCHASTIC_CROSS|adj:S03;pair:EXT-128|%K and %D lengths|intraday timeframe variant of EXT-128
EXT-130|DU|SEM|LONG_SHORT|TREND_THREE_LINE_BREAK_MACD|adj:S03|line-break construction;MACD periods|intraday
EXT-131|U|SEM|LONG_SHORT|VOLATILITY_TREND_ATR|adj:S13;adj:F07|ATR window;comparison window;trend rule|volatility expansion with trend
EXT-132|DU|SEM|LONG_SHORT|VOLATILITY_TREND_ATR|adj:S13;pair:EXT-131|ATR window;comparison window|intraday timeframe variant of EXT-131
EXT-133|DU|SEM|LONG_SHORT|TREND_VORTEX_EMA_ADX|adj:S03|vortex,EMA and ADX settings|intraday
EXT-134|DU|SEM|LONG_SHORT|TREND_VWAP_CROSS|adj:S02|VWAP session or anchor|session VWAP needs intraday bars and an anchor
EXT-135|DU|SEM|LONG_SHORT|TREND_VWAP_CROSS|adj:S02;pair:EXT-134|VWAP session or anchor|intraday timeframe variant of EXT-134
EXT-136|U|SEM|LONG_SHORT|ADAPTIVE_BB_TREND_MR|adj:S06;adj:S04;adj:PMR|band settings;ADX threshold;volume and MACD conditions;ATR stops|adaptive trend/reversion composite with many unstated settings
EXT-137|DU|SEM|LONG_SHORT|ADAPTIVE_VWAP|adj:S06|VWAP anchor;bands;trend filters|intraday
EXT-138|DU|SEM|LONG_SHORT|BREAKOUT_PIVOT|adj:S04|pivot bars;volume;ATR exits|source optimized on 1-second data
EXT-139|U|SEM|LONG_SHORT|REVERSION_VWAP_RSI2|adj:S05;adj:F02;adj:PMR|bar timeframe;VWAP price input;RSI(2) variant;EMA convention|numeric thresholds given but input conventions are not
EXT-140|DU|NEW|LONG_SHORT|ORB_VWAP|-|opening range;HTF RSI threshold;exit|intraday opening range
EXT-141|U|COMP|LONG_SHORT|BREAKOUT_TREND_VOLUME_ATR_EXIT|compo:S04;compo:S02;adj:F04;adj:TRB50;adj:F07;adj:S12|volume filter;trend filter;ATR target;trailing stop;time exit;stop fill semantics|prior-N-bar breakout with trend and volume filters and ATR exits
EXT-142|DU|SEM|LONG_SHORT|MULTI_CONFIRMATION_MOMENTUM|adj:S04;adj:S03|pivot,breakout,VWAP,RSI,MACD settings|intraday multi-indicator
EXT-143|U|SEM|LONG_SHORT|REVERSION_BB_RSI_REENTRY|adj:PMR;adj:S05|band settings;RSI thresholds;exit variant|band re-entry with RSI
EXT-144|U|SEM|LONG_SHORT|BREAKOUT_DONCHIAN_RSI|adj:S04;adj:F04;adj:TRB50|channel length;RSI context;take-profit|rolling high-low channel breakout
EXT-145|U|SEM|LONG_SHORT|REVERSION_RSI_BB_EXIT|adj:S05;adj:PMR|RSI period;band settings;cascade filter|RSI thresholds near 29/71
EXT-146|U|SEM|LONG_SHORT|TREND_MACD_SUPERTREND_DEMA|adj:S03|MACD;Supertrend;DEMA periods;TP/SL|multi-indicator trend
EXT-147|DU|NEW|LONG_SHORT|ORB_VOLUME_CANDLE|neighbor:EXT-140|first-range length;volume and candle filters;risk|intraday opening range
EXT-148|U|SEM|LONG_SHORT|TREND_SUPERTREND_MACD_EMA200|adj:S03;adj:S02|Supertrend settings;MACD periods|multi-indicator trend
EXT-149|FU|SEM|LONG_SHORT|PULLBACK_TREND_RSI|adj:TP5;adj:S05;adj:F02|RSI period;exit rule|crypto instrument; trend pullback
EXT-150|U|SEM|LONG_SHORT|TREND_SUPERTREND_MA_CROSS|adj:S03|Supertrend ATR and factor;MA type;SL/TP|multi-indicator trend
EXT-151|DU|SEM|LONG_SHORT|REVERSION_VWAP_ATR|adj:S06;adj:F06|VWAP length;ATR length;multiplier;risk stop|VWAP distance reversion
EXT-152|U|SEM|LONG_SHORT|TREND_EMA_SUPERTREND|adj:S03|EMA periods;Supertrend settings|multi-indicator trend
EXT-153|U|SEM|LONG_SHORT|TREND_ICHIMOKU_MACD_CMF_TSI|adj:S03;adj:S12|Ichimoku,MACD,CMF,TSI settings|multi-indicator trend
EXT-154|U|SEM|LONG_SHORT|TREND_ICHIMOKU_RSI_MACD|adj:S03|Ichimoku,RSI,MACD settings|multi-indicator trend
EXT-155|U|SEM|LONG_SHORT|TREND_PULLBACK_SUPERTREND_EMA20|adj:TP5;adj:S03|timeframe;Supertrend smoothing;stop/target fill semantics|trend pullback to EMA20 with Supertrend stop and 2R target
EXT-156|F|SEM|LONG_SHORT|TS_MOMENTUM_FUT|adj:S01;adj:AM252|lookbacks;vol target|futures and FX time-series momentum
EXT-157|FDP|NEW|LONG_SHORT|VALUE_EVERYWHERE|-|value measures;ranks|cross-asset valuation
EXT-158|FP|SEM|LONG_SHORT|MOMENTUM_EVERYWHERE|adj:AM252;adj:S01|lookback;ranking|cross-asset momentum
EXT-159|FDPS|SEM|LONG_SHORT|BETTING_AGAINST_BETA|adj:DISCOVERY_01|beta window;leverage|cross-asset low-beta
EXT-160|DPS|NEW|LONG_SHORT|QUALITY_MINUS_JUNK|-|quality components|fundamentals
EXT-161|FDPS|NEW|LONG_SHORT|CARRY_CROSS_ASSET|-|carry measure per asset class|cross-asset
EXT-162|F|SEM|LONG_SHORT|TREND_CENTURY|adj:S01;adj:S02;adj:DSMA|lookbacks;vol target|multi-asset trend
EXT-163|DPS|NEW|LONG_SHORT|PEAD|-|SUE;horizon;liquidity screen|earnings data
EXT-164|DPS|NEW|LONG_SHORT|PEAD_CONDITIONAL|pair:EXT-163|recommendation state;surprise;horizon|analyst data
EXT-165|DPS|NEW|LONG_SHORT|PEAD_FQ1|pair:EXT-163|fiscal-quarter mapping;surprise metric|earnings history
EXT-166|DU|COMP|LONG_HEDGE|TREND_OVERLAY_TSMOM_MA|compo:AM252;compo:S02;adj:F08|T-bill series;and/or combination;hedge instrument|12-month excess return and 12-month MA combined 50/50
EXT-167|DPS|NEW|LONG_SHORT|VALUE_MOMENTUM_SELECTION|-|value and quality measures|fundamentals
EXT-168|DU|NEW|LONG_SHORT|CAL_DATE_HITRATE_SEASONALITY|neighbor:S14|exit and target framework|needs 10/15/20-year per-date history; MQD research history begins in 2016
EXT-169|-|NEW|LONG_FLAT|CAL_SANTA_CLAUS|neighbor:TOM;neighbor:HAL;neighbor:S14|-|long the last 5 sessions of December and the first 2 of January; window lengths stated
EXT-170|-|EXACT|LONG_FLAT|CAL_SELL_IN_MAY|dup:HAL;param:S14|-|long November through April, flat May through October: the registered halloween_nov_apr
EXT-171|PS|NEW|MARKET_NEUTRAL|PAIRS_COINT|-|formation window;hedge ratio;z thresholds|pairs
EXT-172|PS|NEW|MARKET_NEUTRAL|PAIRS_KALMAN|-|noise settings;z-score|pairs
EXT-173|FP|SEM|LONG_SHORT|TREND_MULTI_ASSET|adj:S01;adj:S02|estimator;horizons;vol target|multi-asset trend
EXT-174|U|NEW|OVERLAY|CAL_FILTER_OVERLAY|neighbor:S14|base strategy;seasonal window;hit-rate threshold|an overlay on an unspecified base strategy, not a standalone rule
EXT-175|PS|NEW|LONG_ROTATION|SEASONAL_ROTATION|neighbor:S14|seasonal score;history length|cross-sectional seasonal selection
EXT-176|DU|NEW|LONG_SHORT|CAL_DAILY_HITRATE_TIMING|neighbor:S14|hit-rate rule;intraday entry;exit|needs per-date history and intraday entries
EXT-177|U|PARAM|LONG_SHORT|TREND_SMA_CROSS|param:S03;param:DSMA|fast and slow SMA periods|SMA crossover; periods not stated
EXT-178|U|SEM|LONG_SHORT|TREND_EMA_CROSS|adj:S03;adj:DSMA;neighbor:EXT-177|fast and slow EMA periods|exponential crossover
EXT-179|-|SEM|LONG_SHORT|TREND_MACD_CROSS|adj:S03;adj:DSMA|-|baseline MACD(12,26,9) line/signal crossover is stated; variants are separate identities
EXT-180|U|SEM|LONG_SHORT|REVERSION_RSI|adj:S05;adj:F02|RSI period;thresholds;exit|RSI reversal
EXT-181|U|PARAM|LONG_SHORT|BREAKOUT_NEW_HIGH|param:S04;param:F04;param:TRB50|lookback N;exit|N-period new-high breakout
EXT-182|U|SEM|LONG_SHORT|BREAKOUT_ATR|adj:S13;adj:F07|reference level;ATR length and multiple;stop|ATR breakout
EXT-183|DU|SEM|LONG_SHORT|VWAP_TREND_REVERSION|adj:S02;adj:S06|session VWAP;bands|session VWAP needs intraday bars
EXT-184|FXS|NEW|LONG_SHORT|GRID_TRADING|-|spacing;levels;inventory cap|resting grid orders and inventory on FX/crypto
EXT-185|U|SEM|LONG_SHORT|BREAKOUT_COMPRESSION|adj:F07;adj:S09|compression window;confirmation|compression breakout
EXT-186|FU|SEM|LONG_SHORT|BREAKOUT_BB_VWAP_RSI_ADX|adj:S04|exit rules|crypto instrument
EXT-187|DU|SEM|LONG_SHORT|REVERSION_VWAP_ATR_RSI|adj:S06;adj:F06|VWAP session;time filters;exit|intraday
EXT-188|DU|NEW|LONG_FLAT|BREADTH_DUAL_MOMENTUM|neighbor:AM252|breadth thresholds;breadth source|needs constituent breadth and leveraged ETFs
EXT-189|DU|NEW|LONG_SHORT|ORB_15M|neighbor:EXT-140|risk-reward multiple|intraday opening range
EXT-190|FS|NEW|MARKET_NEUTRAL|CRYPTO_FUNDING_ARB|-|funding thresholds;costs|crypto spot and perpetuals
EXT-191|PS|NEW|MARKET_NEUTRAL|PAIRS_COINT_STOP|-|z entry;stop|pairs
EXT-192|FL|NEW|MODEL|RL_FUTURES|-|architecture;reward|reinforcement learning on futures
EXT-193|FL|NEW|MODEL|RL_CRYPTO_PORTFOLIO|-|architecture;horizon|crypto portfolio weights
EXT-194|FL|NEW|MODEL|RL_LONG_SHORT_PORTFOLIO|-|state tensor;reward|non-US equity universe
EXT-195|LPS|NEW|MODEL|RL_BLACK_LITTERMAN|-|architecture;views|portfolio model
EXT-196|LD|NEW|MODEL|NLP_PRICE_CLASSIFIER|-|embeddings;model|text and price model
EXT-197|LD|NEW|MODEL|MULTIMODAL_AGENT|-|features;policy|multimodal model
EXT-198|LD|NEW|MODEL|RL_HIGH_FREQUENCY|-|policy;turnover penalty|high-frequency model
EXT-199|LPS|NEW|MODEL|TRANSFORMER_PORTFOLIO|-|architecture;constraints|portfolio model
EXT-200|FDU|SEM|LONG_SHORT|PULLBACK_MA_RSI_TP_SL|adj:TP5;adj:S03|MA definitions;context timeframe;fill semantics|crypto, 15-minute signals, rule known only by a screenshot name
"""
