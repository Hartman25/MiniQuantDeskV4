# CLAUDE_WORKFLOW — Layer Model for Claude Code in MiniQuantDesk V4

## Purpose

Claude Code operates on this repo through a four-layer instruction stack. Each layer has a distinct scope and authority. Knowing which layer to use — and which to leave alone — keeps Claude low-token, repo-safe, and non-drifting across sessions.

---

## Layer Model

### 1. System Prompt

**What belongs here:** Model behavior, tool access, tone, output format, safety guardrails. Managed by Anthropic and the Claude Code harness — not by the operator.

**What does NOT belong here:** Repo-specific invariants, domain rules, patch procedures.

**When to rely on it:** It is always active. You cannot inspect or edit it directly.

---

### 2. `CLAUDE.md` — Global Invariants

**What belongs here:** Cross-cutting invariants that apply to every file and every patch in this repo. Examples: fail-closed behavior, determinism requirements, lifecycle chain (outbox→broker→inbox→portfolio), idempotency, patch discipline (one patch per turn, minimal scope).

**What does NOT belong here:** Scoped rules for a single subsystem, patch status tables, commit hashes, open/closed tracking, procedures.

**When to rely on it:** Claude reads this on every session. Write a rule here only if it must hold everywhere, unconditionally.

**Maintenance rule:** Keep it short. If it is growing, extract the subsystem part to a scoped rule file.

---

### 3. `.claude/rules/` — Scoped Constraints

**What belongs here:** Domain constraints that apply to a specific subsystem only. Each file is scoped to a named area (e.g., `db_rules.md` → migrations and persistence; `execution_rules.md` → orchestrator and OMS; `broker_rules.md` → WS transport and cursor management; `gui_rules.md` → operator console and API response types; `audit_repo_truth_rules.md` → closure evidence and status vocabulary).

**What does NOT belong here:** Global invariants (those live in `CLAUDE.md`), patch status, commit history, procedures.

**When to rely on it:** Claude applies the relevant rule file when the stated scope matches the files being changed. A rule file is only active when the work touches its named subsystem.

**Maintenance rule:** Do not duplicate a rule across files. If a rule is in `CLAUDE.md`, do not restate it in a scoped file. Each rule file should reference `CLAUDE.md` for global invariants rather than copying them.

---

### 4. `.claude/skills/` — Procedures

**What belongs here:** Step-by-step procedures for recurring operator actions. Current canonical tracked skills are the five MQD directory skills: `mqd-diagnose/` (deterministic defect isolation, implementation gated on explicit mission/controller authorization), `mqd-test-proof/` (validates whether proof evidence actually proves the claimed invariant), `mqd-review-patch/` (read-only patch/wave review, invoked only at an explicit review request or acceptance boundary — not on every routine edit), `mqd-handoff/` (SHA-bound session handoff; reports status, does not infer or assign it), and `mqd-external-research/` (external docs/papers used as evidence, never as an override of local behavior).

The older flat files `write_patch.md`, `audit_repo.md`, and `verify_proof.md` remain as legacy/local helper notes only. They are not part of the current canonical tracked MQD skill set.

**What does NOT belong here:** Invariants, constraints, status tables, architecture descriptions. Skills do not define acceptance/closure vocabulary — that remains owned by the current mission/controller and the canonical program authority.

**When to rely on it:** Invoke a skill when you need a repeatable, multi-step procedure, per the skill's own stated trigger. Skills reference `CLAUDE.md` and `.claude/rules/` for the rules they enforce — they do not restate rules inline, do not expand their own authority, and do not invoke one another.

**Maintenance rule:** Skills are procedural, not declarative. If a skill is acquiring rules or invariants, extract those to the right rule file. Update this section whenever the tracked skill set or skill type changes.

---

## Instruction Authority

Layers 2–4 above form the repo-controlled authority stack. Resolution order:

current authorized mission/controller
→ `CLAUDE.md`
→ applicable scoped `.claude/rules/`
→ explicitly invoked skill

The System Prompt (Layer 1) sits outside and above this stack; it is managed by Anthropic and the harness and is not redefined or reordered by repo-controlled layers.

---

## Usage Guidance

| Question | Answer |
|---|---|
| Does this rule apply everywhere in the repo? | `CLAUDE.md` |
| Does this constraint apply only to one subsystem? | `.claude/rules/<subsystem>_rules.md` |
| Do I need a step-by-step procedure? | `.claude/skills/<skill>/SKILL.md` |
| Is this a model behavior or tool-access setting? | System prompt (not operator-controlled) |

**Do not overload any one layer.** `CLAUDE.md` must stay short enough to be fully loaded on every session. Rule files must stay local enough to be ignored when irrelevant. Skills must stay procedural enough to be followed without interpretation.

---

## Maintenance Rules

1. **`CLAUDE.md` stays short.** If a rule only applies to one subsystem, it belongs in `.claude/rules/`, not `CLAUDE.md`.
2. **Rule files stay local and non-duplicative.** If a rule is already in `CLAUDE.md`, reference it — do not copy it.
3. **Skills stay procedural.** A skill that grows invariant lists needs a rule file extracted.
4. **No status claims in any layer.** Patch status, commit hashes, and open/closed tables drift. None belong in `CLAUDE.md`, rule files, or skills.
5. **Update this doc when the layer structure changes.** If a new rule category or skill type is introduced, add it here before it is relied upon.
