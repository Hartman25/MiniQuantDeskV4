# EXT-032 independent-review correction (`V4-M1-KISS-EXT032-INDEPENDENT-REVIEW-CORRECTION-01`)

Local, not pushed. Baseline `2651b68`. Four confirmed safety defects from the independent review, each reproduced offline before the fix. Status: SURGICAL CORRECTION APPLIED, NOT INDEPENDENTLY ACCEPTED. Economic execution stays blocked; no adjudication was made, no authorization exists, no secret was provisioned.

| ID | Defect (reproduced) | Correction | Commit |
|---|---|---|---|
| F1 | A rewritten opening with a recomputed public hash, and an unauthenticated appended adjudication, were accepted | Opening entries are pinned by hash in source; an adjudication counts only with an HMAC under `MQK_M1_INCIDENT_ADJUDICATION_KEY` (otherwise ignored and the incident stays pending; a bad signature raises); forward-only transitions; ordered UTC chronology | aa1abaa |
| F2 | `ADJUDICATED_HOLDOUT_CONSUMED` cleared the incident veto like `PRESERVED` | Blocking states are pending and consumed; Promotion/Paper authorization, the review and the guard report treat a consumed window as never untouched; only an authenticated PRESERVED clears | aa1abaa |
| F3 | `stage_check` (and `gate`) executed the mutable `MQK_M1_CLI` binary with no authorization | Binary execution requires an authorization whose signed `cli_sha256` equals the executed binary (new `native_identity_resolution` class); `check` stays declaration-only offline unless so authorized; `gate` is no longer read-only | affb125 |
| F4 | An ordinary spawned child did not inherit the parent's audit guard and could read a fake `.env.local` and connect | `sitecustomize` guard injected through a guarded `Popen` into any child environment; isolation flags, network clients, unfollowable spawn APIs and secret files handed to non-Python children are refused; audit backstop; child attempts logged and failing the owning test and the CI proof | 7e80b62, 9c5dcc7 |

## Authority limits (unchanged in kind)
HMAC secrets are operator-held shared secrets, not non-repudiation. A hash pin in source makes a rewrite a reviewed code change, not impossible. The audit hook cannot see raw syscalls (`ctypes`) or children that are not Python and not a screened tool; the one binary the runner executes is hash-pinned, and tests use stubs. A kernel network namespace (for example `unshare -n`) would close those residuals and was not added.

## Operator decisions still open
Adjudication of `HOA-KISS-EXT032-01`; who holds `MQK_M1_INCIDENT_ADJUDICATION_KEY` and `MQK_M1_STAGE_AUTH_KEY`; the corporate-action discovery acknowledgement; a canonical Promotion report producer; SAB-1.

## Second adversarial sweep
1. Direct readers of the ledger file besides the module: none outside tests. FIXED+PROVEN (single seam).
2. Every consumer of incident state (stage authorization, review, guard report): updated to blocking states, covered by tests. FIXED+PROVEN.
3. A forged signature when the secret is absent: ignored (pending), not accepted. ALREADY CORRECT+PROVEN by the new design.
4. Binary execution from other stages (`register`, `trials`, `backtest`, `finalize`, `review`): all pinned. FIXED+PROVEN.
5. Residual time-of-check/time-of-use between hashing and executing the binary: LOW, documented.
6. Python children via `multiprocessing` spawn, shells and grandchildren with cleared environments: guarded, PROVEN.
7. `os.posix_spawn` used inside CPython's own `Popen`: allowed only inside the guarded wrapper (found and fixed during the work).
8. Non-Python tools handed a `.env*` argument: refused (added in the sweep). FIXED+PROVEN.
9. Raw-syscall and non-Python network egress: BLOCKED by capability (needs a kernel namespace), recorded.

## Follow-up (`V4-M1-KISS-EXT032-FINAL-SAFETY-FOLLOWUP-02`)
Supersedes sweep items 6 and 8 (command-line scanning is no longer an isolation boundary).

1. **Executable boundary (F3).** `run_batch._run_cli` calls `stage_authorization.authorize_native_execution`, which re-verifies at execution time: HMAC signature, declaration identity, the class the active stage needs (`check` needs `native_identity_resolution`; effectful stages their own class; other read-only stages none), validity window, incident acknowledgement, and the `cli_sha256` pin. `_ACTIVE` is only a hint; a forged mapping fails the signature. `staged()` restores its caller's state (nested stages neither inherit nor end it). `_save_index` re-verifies likewise; `run_campaign` (no stage gate) refuses any declaration that is not a pinned historical one.
2. **Child launches (F4).** The offline guard permits only (a) this interpreter (same file as `sys.executable`/its base executable, by resolved path, not by name), started without `-I/-S/-E` (option arguments of `-W/-X` are skipped when parsing) and with the guard directory first on `PYTHONPATH`; (b) an executable a test vouches for with `allow_executable` (resolved path plus content hash). Shells, `env`/`nice`/`timeout`/`xargs`, other interpreters and every other program are refused as `unsupported_launcher`, whatever their command line says. The same policy runs in the audit-hook backstop.
3. **Evidence collection.** The audit sink is fixed when the guard installs and re-imposed on every child environment (a child-supplied `MQK_NETGUARD_LOG` is replaced, never re-read). A launch whose sink is not writable is refused. Every guarded Python child writes `guard_ready` from `sitecustomize` or exits (97); the parent records `child_launched`, and the per-test fixture, the session summary and the CI proof guard fail on a launched child without `guard_ready`. Children cannot truncate, move, delete or append to the sink (`sink_tamper`).

Verified platform behaviour (CPython 3.13, Linux): a `sitecustomize.py`/`usercustomize.py` in `cwd` (`-c`), in the script directory, or earlier on a child-supplied `PYTHONPATH` does not displace the guard.

Remaining limits (recorded, not fixed): non-Python programs cannot be sandboxed without an OS namespace, so they are refused rather than guarded; a vouched stub runs unguarded by design (test-only); raw syscalls, `ctypes`, `_posixsubprocess.fork_exec` and native extensions are outside the audit hook; a malicious child can still exhaust or flood the sink file; the hash-to-exec window for the pinned binary and for vouched stubs is not closed; a caller that can forge both `_ACTIVE` and a valid signature holds the operator secret.

## Sink follow-up (`V4-M1-KISS-EXT032-IR-SINK-FOLLOWUP-03`)
Supersedes the "sink fixed at install" wording above: `install(sink=...)` and `sink_to()` could re-point the sink, so a child's denied attempt could be written somewhere the session never audits.

- **Root vs copies.** The first `install` that names a sink (the outermost process: `conftest.py`) fixes the immutable ROOT audit sink; a guarded child inherits it from its environment. A later `install(sink=<other>)` raises `SinkRebindError`; naming the same file (also through a symlink) is a no-op. Every process writes every row (`child_launched`, `guard_ready`, each denied attempt) to the root first. `sink_to(path)` only adds a validated diagnostic COPY (also inherited by children and grandchildren) and can neither replace nor hide anything from the root.
- **Sink validity.** A sink must be a regular file (`/dev/null`, directories, FIFOs and links to them are refused), not named like a secret file, and is bound to the (device, inode) it had when validated; every append re-checks that identity and a required destination that is missing, replaced or unwritable fails closed (launch refused; a child that cannot report exits 97; the parent counts `sink_integrity_errors`). Relative paths are pinned to their absolute file.
- **Reading.** `sink_rows` raises `SinkCorrupt` for a missing sink, a malformed or non-object row, a mistyped lineage field or a partial final row; it never reports an unreadable sink as zero attempts. The session summary carries `sink_integrity_errors`, which the CI proof guard requires to be 0.
- **Attribution.** Lineage is rebuilt from the sink with launch tokens (not pids): each wrapped launch gets a fresh id, the child announces `guard_ready` with it, and the parent records `child_launched` (expected only if launched inside its own `expect_denied`). A descendant is deliberate only if every path to it passed through an expected launch; a writer's own `expected` flag is ignored. A pytest session started inside a test audits its own subtree of the inherited root; the outermost session also counts attempts by processes outside any known lineage.
- **Deliberate probes** are marked per probe with `expect_denied()` around the launch; `sink_to`/`run_guarded` scopes exempt nothing.
- **Launch-policy proof.** The wrapper refusal and the audit-hook backstop each have their own test (wrapper: the launch never reaches `Popen._execute_child`; backstop: refusal from `_hook` with the wrapper bypassed), plus a control showing the same launch runs when the shared policy is neutralised.

Threat model (not a sandbox): the guard is an in-process Python audit hook plus file/identity checks. A hostile process with arbitrary native code, `ctypes`, raw syscalls, the ability to edit `_netguard` internals or to write the root through a path the hook does not see can still defeat it; that needs an OS namespace/sandbox and is an operator policy decision, not something this code claims.
