# Repowise Pilot 01 — Read-Only Supplementary Repository Intelligence

Component A of `V4-MQD-FOUR-WAY-ASSET-NEUTRAL-INTEGRATION-ISOLATED-01`.

Status: **PILOT COMPLETE — MIXED BENEFIT, KEPT DISABLED BY DEFAULT**.

## 1. Pinned source (operator-authorized)

The mission's planning-time commit reference
(`dfe721794c374eeb514f8708cc2ce7554c0b8e89`) was verified against the GitHub
API: it is a real `repowise-dev/repowise` commit (2026-10-09, a web-dashboard
i18n change — `messages/zh-CN.json` + language switcher), but it is **not**
a tagged release and does not correspond to any published package version.
It postdates the `v0.55.0` tag and was never itself released.

Operator decision: pin to the current published release instead.

| Field | Value |
|---|---|
| Package | `repowise` (PyPI) |
| Version | `0.55.0` (explicit pin, not a floating spec) |
| Release tag | `v0.55.0` |
| Tag commit | `e829775bfd18f22ff2f9522d514d3b94c1fde2b6` |
| Wheel | `repowise-0.55.0-py3-none-any.whl` |
| Wheel SHA-256 | `5401c57dd3ce4707e876c7ca8e9ebd21a67a9982afbde09bc7d44014937c3eb9` |
| License | `AGPL-3.0-or-later` |
| Verified | Independently re-derived (own `sha256sum` + GitHub tags lookup), then cross-checked against operator-supplied values — all three match. |

AGPL-3.0 obligations do not propagate into MQD: Repowise is installed
standalone (pip, isolated venv), never vendored, and used purely
internally/locally — no modified distribution, no network service exposing
it to third parties.

## 2. Installation (isolated, reproducible)

Installed into a dedicated venv **outside both the original repo and this
clone** (OS temp dir), never inside either repository:

```
python -m venv <temp>/mqd-repowise-pilot-venv
<venv>/Scripts/python.exe -m pip install repowise==0.55.0
```

Verified post-install: `pip show repowise` reports version `0.55.0`;
re-hashed wheel matches `5401c57...`; no `direct_url.json` (installed
cleanly from the public PyPI index, no private/custom source).

## 3. Safe initialization — exact flags (verified against the real installed `--help`, not assumed)

The mission's assumed flag list was confirmed accurate against the actual
CLI (`repowise init --help`), with one correction: there is no literal
`--no-llm` flag. The functional equivalent is `--no-prose` (plus never
passing `--provider`), which renders the structural wiki with **no model
and no key**.

Exact command used, run only against this isolated clone:

```
REPOWISE_SKIP_EDITOR_SETUP=1 REPOWISE_NO_SAVE_KEY=1 DO_NOT_TRACK=1 \
repowise telemetry disable

REPOWISE_SKIP_EDITOR_SETUP=1 REPOWISE_NO_SAVE_KEY=1 DO_NOT_TRACK=1 \
repowise init --yes --no-prose --no-editor-setup --no-hook \
  --no-distill-hook --no-claude-md --no-agents --no-codex \
  --no-save-key --no-workspace --mode fast --progress json .
```

- `--mode fast`: graph + essential git only, no per-file blame/co-change,
  no LLM docs — bounded pilot footprint, not a full production index.
- `--editor-setup`/`--hook` default **on** and would write a machine-wide
  Claude Code MCP entry, project-local `.mcp.json`/`.claude/CLAUDE.md`
  blocks, and a post-commit git hook. All disabled here.
- `--distill-hook` defaults to "ask when interactive, skip otherwise";
  explicit `--no-distill-hook` removes any ambiguity under `--yes`.
- `REPOWISE_SKIP_EDITOR_SETUP=1` is a second, env-level guarantee that wins
  even if a flag were mistakenly reversed.

### Negative controls — safety requirements verified, not assumed

| Requirement | Verification | Result |
|---|---|---|
| No external LLM calls | `--no-prose`, no `--provider` passed; init log shows no provider prompt, no key I/O | PASS |
| No paid providers | same as above; `anthropic`/`openai`/`google-genai`/`litellm` are installed dependencies but never invoked | PASS |
| No telemetry | `repowise telemetry status` → `disabled, reason: DO_NOT_TRACK is set` | PASS |
| No Git hook installed | `.git/hooks/` contains only stock `*.sample` files, no `post-commit` | PASS |
| No in-repo/editor config written | repo `git status --short` after init shows **only** the `.gitignore` edit made by this patch — no `.mcp.json`, `.vscode/mcp.json`, `.vscode/extensions.json`, `CLAUDE.md`, or `AGENTS.md` created/modified | PASS |
| No *meaningful* global config change | `repowise uninstall --all --dry-run` revealed a small machine-wide `C:\Users\Zacha\.repowise\` directory (127 bytes total) the CLI itself created outside both repos: `editor-migrations` (a bookkeeping marker recording that the `claude_code_hooks`/`codex_rewrite_hook` migration checks ran and found nothing to migrate — consistent with `--no-editor-setup`/`--no-codex`, not an actual hook install) and `platform.json` (anonymous telemetry id + `telemetry_enabled: false`, confirming the opt-out took effect machine-wide). No credentials, no editor wiring, no hook. Disclosed here rather than omitted; removable via `repowise uninstall --all` (see §6) | PASS, with disclosed exception (inert bookkeeping only) |
| No source upload / account creation | `publish`/`login`/`whoami` commands exist but were never invoked | PASS (by omission) |
| Local bounded index, excluded from Git | `.repowise/` (425 MB) added to `.gitignore` before first run; `git status --ignored` confirms it is ignored | PASS |
| Original repo untouched | original repo `git status --short` clean, HEAD unchanged, at every checkpoint | PASS |

## 4. Benchmark — 5 bounded tasks, same source revision (`a49cae1f9a28`, this clone's HEAD)

Existing MQD MCP baseline (`mqk_readonly` + Srclight) was run against the
**original repo's** HEAD (`94d7fbc7`) because those tools are hardwired to
that repo and the isolation contract forbids redirecting them into this
clone. Repowise was run only against **this clone's** HEAD (`a49cae1f9a28`,
origin/main). The two revisions are a few commits apart; this is a
disclosed limitation of the comparison, not elided.

| # | Task | Srclight / mqk_readonly result | Repowise result | Finding |
|---|---|---|---|---|
| 1 | Callers of `ResearchResultStore.register_trial` | `get_callers("register_trial")` (name-only): 25 results, but **conflates** the Python method with an unrelated same-named Rust function in `mqk-promotion` test files (homonym collision) | `context ...storage.py::register_trial --include callers` (symbol-qualified): 20 results, correctly scoped to only the Python symbol; found 3 genuine production callers (`economic_registry_integration.py`, `native_signal_registry_integration.py`, `registry_integration.py`) that Srclight's list did not surface at all | Mixed: Repowise avoided a real cross-language homonym conflation and found 3 production callers Srclight missed; Srclight found `m1_native_trend_campaign` callers Repowise's list omitted |
| 2 | Impact of changing `max_drawdown_pct_from_equity` | `get_dependents(transitive=true)`: 113 dependents across Rust tests, Python experiments, production call sites | `context ...::max_drawdown_pct_from_equity --include callers`: only 1 caller (`compute_capital_fraction_benchmark`, same-file); tool's own caveat: *"Call sites the resolver could not bind are not counted, so an empty list means no resolved edge reaches this symbol, not proof that nothing calls it"* | Clear loss for Repowise: its Rust cross-file/test-file call resolution is materially weaker than Srclight's transitive graph for this free function |
| 3 | Tests covering `compute_capital_fraction_benchmark` | `get_tests_for(...)`: **0 tests** (confirmed false negative — native `grep` shows real coverage in `scenario_capital_fraction_benchmark_01.rs` and `scenario_capital_fraction_benchmark_identity_01.rs`) | `context ...` → `docs.used_by`: correctly lists **both** real test files | Clear win for Repowise on this exact task — it caught the case where Srclight's heuristic test-matcher produced a false negative |
| 4 | Retrieve architectural decision for `storage.py` | Not directly comparable (Srclight has no decision-record feature) | `why --target storage.py`: 0 formal decisions recorded (consistent repo-wide — `Decisions: 0` at index time), falls back to real git archaeology: primary author, 8 commits, first/last commit dates, key commit messages | Repowise offers a capability Srclight doesn't have; usefulness is limited here only because MQD has no formal decision records to retrieve — correctly reported as absent rather than fabricated |
| 5 | Detect a stale/dead path | No direct MQD-MCP equivalent exercised | `dead-code --format json`: 12 unreachable files, 155 unused exports, each with confidence score and `safe_to_delete` flag; flagged e.g. `core-rs/crates/mqk-config/src/consumption.rs` and `mqk-testkit/src/execution_simulator.rs` as zero-importer files | Real, structured output with confidence scoring; not independently verified further (out of this mission's scope to act on) — recorded as capability evidence only |

### Timing / overhead

| Phase | Elapsed |
|---|---|
| Repowise `init --mode fast` (one-time index build, 2,312 files / 47,492 symbols) | 8m 10s |
| Repowise 5-task benchmark query batch | ~105s |
| Srclight/mqk_readonly 4-call baseline batch (tasks 1–3 + one extra) | ~18s |
| Local index size | 425 MB (`.repowise/`, gitignored, never committed) |

Token/cost usage: not applicable — zero LLM calls were made in either tool
during this benchmark (`--no-prose`, no provider configured).

## 5. Acceptance

**Mixed, task-dependent benefit — no blanket improvement.** Repowise's
symbol-qualified queries avoided a real homonym-conflation bug in Srclight's
name-only search (Task 1) and caught a confirmed false negative in
Srclight's test-coverage heuristic (Task 3). It also exposes capabilities
MQD's existing MCPs lack entirely (`why` git archaeology, `dead-code` with
confidence scoring, `risk` change-review scoring). But its Rust call-graph
resolution is materially weaker than Srclight's transitive dependents graph
for free functions with indirect/test-only call sites (Task 2, and by
extension Task 3's direct `callers` field, which was also empty there).

Per the mission's acceptance rule ("do not retain an active integration if
it offers no demonstrated benefit" / "a measured lack of benefit is
acceptable evidence"): the benefit is real but narrow and task-specific,
not a general replacement for the existing discovery order. Repowise is
**not** wired into any MCP config, hook, or default workflow. It remains an
optional, manually-invoked, isolated pilot.

## 6. Disable / uninstall

Nothing was installed into either repository's tracked files, global Claude
Code config, or git hooks, so there is nothing in-repo to revert beyond this
clone's local `.repowise/` index directory, its `.gitignore` entry, and the
small inert machine-wide `C:\Users\Zacha\.repowise\` bookkeeping/telemetry
directory noted in §3.

Dry-run plan verified (`repowise uninstall --all --dry-run --format json .`):
confirms no `.claude/CLAUDE.md` REPOWISE block and no `AGENTS.md`
REPOWISE_AGENTS block exist to remove (proving `--no-claude-md`/`--no-agents`
worked), lists the 445 MB local index and the 127-byte global state dir as
the only real removal targets, and notes the package itself is not its own
to remove (`pip uninstall repowise` — i.e. delete the isolated venv, below).

To remove the local index and the global bookkeeping dir:

```
<venv>/Scripts/repowise.exe uninstall --all --path <clone-path>
```
(`--keep-index` instead of `--all` would keep the index for a future pilot
while removing any agent wiring — none was written here, so `--all` is
equivalent and simplest.)

To remove the standalone pilot install entirely: delete the isolated venv
directory (`<temp>/mqd-repowise-pilot-venv`). Nothing else on the machine
references it.

## 7. Discovery/tool-usage order followed

`mqk_readonly` → Srclight → Graft (unavailable: MCP connection timed out at
session start, documented, not retried after the one failure) → Repowise →
native `git`/grep verification, per the mandatory MCP/skill-first order.
