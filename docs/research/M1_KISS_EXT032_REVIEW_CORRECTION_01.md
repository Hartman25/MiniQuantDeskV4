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
