pub enum RunMode { Backtest, Paper, Live }

pub fn consumed_pointers(mode: RunMode) -> &'static [&'static str] {
    match mode {
        RunMode::Backtest => BACKTEST,
        RunMode::Paper => PAPER,
        RunMode::Live => LIVE,
    }
}

static BACKTEST: &[&str] = &[
    "/runtime/mode",
    "/data/timeframe",
    "/backtest",              // treat as subtree root if you consume whole section
    "/execution/slippage",    // etc
];

static PAPER: &[&str] = &[
    "/runtime/mode",
    "/broker",
    "/risk",
    "/execution",
];

static LIVE: &[&str] = &[
    "/runtime/mode",
    "/broker",
    "/risk",
    "/execution",
    "/integrity",
    "/reconcile",
];
