#!/usr/bin/env bash
# Mutation-negative proof for scripts/guards/check_m1_experiment_proof.py: a broken guard, a missing or
# renamed test module, a skipped/failed test, a trimmed run and a missing/dirty offline-guard summary must each
# make the real guard exit non-zero; an intact run must pass. Fixtures are synthetic JUnit/JSON files; the real
# guard script is invoked exactly as CI invokes it.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GUARD="$REPO_ROOT/scripts/guards/check_m1_experiment_proof.py"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FAILURES=0
pass() { echo "  PASS  [$1] $2"; }
fail() { echo "  FAIL  [$1] $2"; FAILURES=$((FAILURES + 1)); }

make_junit() { # out, optional mutation: skip|fail|drop|trim|none
  python3 - "$1" "${2:-none}" "$GUARD" <<'PY'
import importlib.util, sys
out, mutation, guard = sys.argv[1:4]
spec = importlib.util.spec_from_file_location("g", guard); g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)
mods = list(g.REQUIRED_MODULES)
if mutation == "drop":
    mods = mods[1:]
per = 40 if mutation != "trim" else 3
cases = []
for m in mods:
    for i in range(per):
        body = ""
        if mutation == "skip" and m == mods[0] and i == 0:
            body = "<skipped message='x'/>"
        if mutation == "fail" and m == mods[0] and i == 0:
            body = "<failure message='x'/>"
        cases.append(f"<testcase classname='{m}' name='t{i}'>{body}</testcase>")
open(out, "w").write("<testsuites><testsuite>" + "".join(cases) + "</testsuite></testsuites>")
PY
}

run_guard() { python3 "$GUARD" "$1" "$2" >"$WORK/out.txt" 2>&1; echo $?; }

echo ""
echo "check_m1_experiment_proof mutation-negative tests"
GOOD='{"attempted_total": 7, "unexpected_attempts": 0, "unexpected_child_attempts": 0, "uninitialized_children": 0}'

make_junit "$WORK/ok.xml"; echo "$GOOD" > "$WORK/ok.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/ok.json")" = 0 ] && pass M1P-A "intact run passes" || fail M1P-A "intact run rejected: $(cat "$WORK/out.txt")"

for m in skip fail drop trim; do
  make_junit "$WORK/$m.xml" "$m"
  [ "$(run_guard "$WORK/$m.xml" "$WORK/ok.json")" != 0 ] && pass "M1P-$m" "mutation '$m' is rejected" || fail "M1P-$m" "mutation '$m' passed"
done

echo '{"attempted_total": 7, "unexpected_attempts": 1, "unexpected_child_attempts": 0, "uninitialized_children": 0}' > "$WORK/dirty.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/dirty.json")" != 0 ] && pass M1P-dirty "an unexpected network attempt is rejected" || fail M1P-dirty "dirty summary passed"
echo '{"attempted_total": 0, "unexpected_attempts": 0, "unexpected_child_attempts": 0, "uninitialized_children": 0}' > "$WORK/inert.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/inert.json")" != 0 ] && pass M1P-inert "an offline guard that recorded no probe (not installed) is rejected" || fail M1P-inert "inert guard passed"
echo '{"attempted_total": 7, "unexpected_attempts": 1, "unexpected_child_attempts": 1, "uninitialized_children": 0}' > "$WORK/childdirty.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/childdirty.json")" != 0 ] && pass M1P-child "an unexpected attempt by a spawned child is rejected" || fail M1P-child "child attempt passed"
echo '{"attempted_total": 7, "unexpected_attempts": 0, "unexpected_child_attempts": 0}' > "$WORK/nochildkey.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/nochildkey.json")" != 0 ] && pass M1P-nochildkey "a summary without child accounting (guard predates child inheritance) is rejected" || fail M1P-nochildkey "summary lacking child accounting passed"
echo '{"attempted_total": 7, "unexpected_attempts": 1, "unexpected_child_attempts": 0, "uninitialized_children": 1}' > "$WORK/ghost.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/ghost.json")" != 0 ] && pass M1P-ghost "a spawned child that never initialized the guard is rejected" || fail M1P-ghost "uninitialized child passed"
echo '{"attempted_total": 7, "unexpected_attempts": 0, "unexpected_child_attempts": 0}' > "$WORK/noghostkey.json"
[ "$(run_guard "$WORK/ok.xml" "$WORK/noghostkey.json")" != 0 ] && pass M1P-noghostkey "a summary without child-initialization accounting is rejected" || fail M1P-noghostkey "summary lacking initialization accounting passed"
[ "$(run_guard "$WORK/ok.xml" "$WORK/absent.json")" != 0 ] && pass M1P-nosummary "a missing summary is rejected" || fail M1P-nosummary "missing summary passed"
[ "$(run_guard "$WORK/absent.xml" "$WORK/ok.json")" != 0 ] && pass M1P-nojunit "a missing junit report is rejected" || fail M1P-nojunit "missing junit passed"

# the CI workflow must actually invoke the guard and every required module must be on the pytest command line
CI="$REPO_ROOT/.github/workflows/ci.yml"
grep -q "check_m1_experiment_proof.py" "$CI" && pass M1P-ci-guard "ci.yml invokes the guard" || fail M1P-ci-guard "ci.yml does not invoke the guard"
grep -q "test_m1_experiment_proof_guard.sh" "$CI" && pass M1P-ci-neg "ci.yml runs this mutation-negative proof" || fail M1P-ci-neg "ci.yml does not run this proof"
python3 - "$GUARD" "$CI" <<'PY' && pass M1P-ci-modules "every required module is selected by the CI pytest command" || fail M1P-ci-modules "a required module is not selected by the CI command"
import importlib.util, re, sys
spec = importlib.util.spec_from_file_location("g", sys.argv[1]); g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)
ci = open(sys.argv[2]).read()
block = ci[ci.index("M1 KISS experiment guards"):]
block = block[:block.index("check_m1_experiment_proof.py")]
missing = []
for m in g.REQUIRED_MODULES:
    path = m.replace(".", "/") + ".py"
    directory = path.rsplit("/", 1)[0]
    if path not in block and directory not in block:
        missing.append(m)
sys.exit(1 if missing else 0)
PY
grep -q "MQK_NETGUARD_SUMMARY" "$CI" && pass M1P-ci-netguard "ci.yml requests the offline-guard summary" || fail M1P-ci-netguard "ci.yml does not request the summary"
grep -q "openpyxl==" "$CI" && pass M1P-ci-openpyxl "ci.yml installs the workbook reader so nothing skips" || fail M1P-ci-openpyxl "no openpyxl install"

echo ""
if [ "$FAILURES" -eq 0 ]; then echo "All check_m1_experiment_proof mutation-negative tests passed."; exit 0; fi
echo "$FAILURES failure(s)."; exit 1
