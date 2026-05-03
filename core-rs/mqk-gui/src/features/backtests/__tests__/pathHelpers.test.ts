import test from "node:test";
import assert from "node:assert/strict";
import {
  classifyArtifactPathInput,
  buildRepoRelativePath,
  buildMd1DSymbolPath,
  FIXTURE_BARS_SEGMENTS,
  BACKTEST_OUT_SEGMENTS,
  MD_BACKUP_1D_SEGMENTS,
  MD_INGEST_SEGMENTS,
} from "../pathHelpers.ts";

// ---------------------------------------------------------------------------
// classifyArtifactPathInput
// ---------------------------------------------------------------------------

test("classifyArtifactPathInput: blank string returns blank", () => {
  assert.deepEqual(classifyArtifactPathInput(""), { kind: "blank" });
});

test("classifyArtifactPathInput: whitespace-only returns blank", () => {
  assert.deepEqual(classifyArtifactPathInput("   "), { kind: "blank" });
});

test("classifyArtifactPathInput: .csv path returns csv_file", () => {
  assert.deepEqual(
    classifyArtifactPathInput("C:\\path\\to\\bkt4_bars.csv"),
    { kind: "csv_file" },
  );
});

test("classifyArtifactPathInput: .CSV uppercase extension returns csv_file", () => {
  assert.deepEqual(
    classifyArtifactPathInput("/data/bars.CSV"),
    { kind: "csv_file" },
  );
});

test("classifyArtifactPathInput: equity_curve.csv returns csv_file", () => {
  assert.deepEqual(
    classifyArtifactPathInput("exports\\backtests\\run-abc\\equity_curve.csv"),
    { kind: "csv_file" },
  );
});

test("classifyArtifactPathInput: folder path returns folder", () => {
  assert.deepEqual(
    classifyArtifactPathInput("C:\\exports\\backtests\\7273b99b-bbed-58be-9db4-615e9bfaf901"),
    { kind: "folder" },
  );
});

test("classifyArtifactPathInput: unix-style folder path returns folder", () => {
  assert.deepEqual(
    classifyArtifactPathInput("/home/user/exports/backtests/run-abc"),
    { kind: "folder" },
  );
});

test("classifyArtifactPathInput: path ending with .json returns folder (not csv_file)", () => {
  assert.deepEqual(
    classifyArtifactPathInput("/tmp/manifest.json"),
    { kind: "folder" },
  );
});

// ---------------------------------------------------------------------------
// buildRepoRelativePath
// ---------------------------------------------------------------------------

test("buildRepoRelativePath: null repoRoot returns null", () => {
  assert.equal(buildRepoRelativePath(null, "exports", "backtests"), null);
});

test("buildRepoRelativePath: empty repoRoot returns null", () => {
  assert.equal(buildRepoRelativePath("", "exports", "backtests"), null);
});

test("buildRepoRelativePath: Windows root + segments uses backslash", () => {
  const result = buildRepoRelativePath(
    "C:\\Users\\dev\\MiniQuantDeskV4",
    ...BACKTEST_OUT_SEGMENTS,
  );
  assert.equal(result, "C:\\Users\\dev\\MiniQuantDeskV4\\exports\\backtests");
});

test("buildRepoRelativePath: Unix root + segments uses forward slash", () => {
  const result = buildRepoRelativePath(
    "/home/dev/MiniQuantDeskV4",
    ...BACKTEST_OUT_SEGMENTS,
  );
  assert.equal(result, "/home/dev/MiniQuantDeskV4/exports/backtests");
});

test("buildRepoRelativePath: md_backup 1D path on Windows", () => {
  const result = buildRepoRelativePath(
    "C:\\repo",
    ...MD_BACKUP_1D_SEGMENTS,
  );
  assert.equal(result, "C:\\repo\\exports\\md_backup\\1D");
});

test("buildRepoRelativePath: fixture bars path on Windows", () => {
  const result = buildRepoRelativePath(
    "C:\\repo",
    ...FIXTURE_BARS_SEGMENTS,
  );
  assert.equal(
    result,
    "C:\\repo\\core-rs\\crates\\mqk-backtest\\tests\\fixtures\\bkt4_bars.csv",
  );
});

test("buildRepoRelativePath: no segments returns just the root", () => {
  assert.equal(buildRepoRelativePath("C:\\repo"), "C:\\repo");
});

// ---------------------------------------------------------------------------
// buildMd1DSymbolPath
// ---------------------------------------------------------------------------

test("buildMd1DSymbolPath: null repoRoot returns null", () => {
  assert.equal(buildMd1DSymbolPath(null, "AAPL"), null);
});

test("buildMd1DSymbolPath: empty repoRoot returns null", () => {
  assert.equal(buildMd1DSymbolPath("", "AAPL"), null);
});

test("buildMd1DSymbolPath: blank symbol returns null", () => {
  assert.equal(buildMd1DSymbolPath("C:\\repo", ""), null);
});

test("buildMd1DSymbolPath: Windows repo root builds correct path", () => {
  const result = buildMd1DSymbolPath("C:\\Users\\dev\\MiniQuantDeskV4", "AAPL");
  assert.equal(
    result,
    "C:\\Users\\dev\\MiniQuantDeskV4\\exports\\md_backup\\1D\\AAPL_1D.csv",
  );
});

test("buildMd1DSymbolPath: Unix repo root builds correct path", () => {
  const result = buildMd1DSymbolPath("/home/dev/MiniQuantDeskV4", "aapl");
  assert.equal(
    result,
    "/home/dev/MiniQuantDeskV4/exports/md_backup/1D/AAPL_1D.csv",
  );
});

test("buildMd1DSymbolPath: lowercase symbol is uppercased", () => {
  const result = buildMd1DSymbolPath("C:\\repo", "spy");
  assert.equal(result, "C:\\repo\\exports\\md_backup\\1D\\SPY_1D.csv");
});

test("buildMd1DSymbolPath: does not contain hardcoded user home path", () => {
  const result = buildMd1DSymbolPath("C:\\repo", "AAPL");
  assert.ok(result !== null, "should be non-null with valid inputs");
  assert.ok(!result!.includes("Zacha"), "must not contain hardcoded user path");
  assert.ok(!result!.includes("Users\\Zacha"), "must not contain hardcoded user path");
});

// ---------------------------------------------------------------------------
// MD_INGEST_SEGMENTS (DATA-INGEST-GUI-RUNNER-01)
// ---------------------------------------------------------------------------

test("MD_INGEST_SEGMENTS builds correct Windows path via buildRepoRelativePath", () => {
  const result = buildRepoRelativePath("C:\\repo", ...MD_INGEST_SEGMENTS);
  assert.equal(result, "C:\\repo\\exports\\md_ingest");
});

test("MD_INGEST_SEGMENTS builds correct Unix path via buildRepoRelativePath", () => {
  const result = buildRepoRelativePath("/home/dev/repo", ...MD_INGEST_SEGMENTS);
  assert.equal(result, "/home/dev/repo/exports/md_ingest");
});

test("MD_INGEST_SEGMENTS returns null for null repoRoot", () => {
  assert.equal(buildRepoRelativePath(null, ...MD_INGEST_SEGMENTS), null);
});

test("MD_INGEST_SEGMENTS does not contain hardcoded user home path", () => {
  const result = buildRepoRelativePath("C:\\repo", ...MD_INGEST_SEGMENTS);
  assert.ok(result !== null);
  assert.ok(!result!.includes("Zacha"), "must not contain hardcoded user path");
});
