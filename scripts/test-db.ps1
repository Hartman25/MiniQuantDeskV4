& "$PSScriptRoot\dev-shell.ps1"
cargo test -p mqk-db --test scenario_inbox_apply_atomic_recovery -- --nocapture
