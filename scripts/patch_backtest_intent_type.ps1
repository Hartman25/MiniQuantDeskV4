param(
  [string]$RepoRoot = (Resolve-Path ".").Path
)

$engine = Join-Path $RepoRoot "core-rs\crates\mqk-backtest\src\engine.rs"
if (!(Test-Path $engine)) {
  Write-Error "engine.rs not found at: $engine"
  exit 1
}

$txt = Get-Content -Raw $engine

# Fix: is_intent_risk_reducing should accept ExecutionIntent (current pipeline emits ExecutionIntent)
$txt2 = $txt -replace "fn\s+is_intent_risk_reducing\(\&self,\s*intent:\s*\&mqk_execution::OrderIntent\)",
                      "fn is_intent_risk_reducing(&self, intent: &mqk_execution::ExecutionIntent)"

if ($txt2 -eq $txt) {
  Write-Warning "No change made. Signature pattern not found (file may already be patched or differs)."
} else {
  Set-Content -NoNewline -Encoding UTF8 $engine $txt2
  Write-Host "Patched: $engine"
}
