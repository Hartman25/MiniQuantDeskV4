Write-Host "== Format check =="
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== Clippy strict =="
cargo clippy --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== Workspace tests =="
cargo test --workspace --all-targets --no-fail-fast
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== DB integration tests =="

if (-not $env:MQK_DATABASE_URL) {
    $env:MQK_DATABASE_URL = "postgres://postgres:postgres@localhost/mqk_test"
}

cargo test -p mqk-db `
  --features testkit `
  -- `
  --include-ignored `
  --test-threads=1 `
  --nocapture

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== CI GATE PASSED =="