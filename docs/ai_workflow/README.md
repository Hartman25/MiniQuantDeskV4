# MiniQuantDesk AI Workflow Pack

This folder is a **workflow and prompt layer** for AI-assisted work inside MiniQuantDesk V4.
It is **not** a replacement for project truth.

## What this folder is for

Use this folder to reduce:
- repo rediscovery
- prompt drift
- accidental scope widening
- false closure claims
- patch packets that are too vague to be safe

Use it to improve:
- patch scoping
- file-grounded claims
- repeatable AI prompts
- operator ledger continuity
- clean handoff between ChatGPT, Claude, Codex, and other tools

## What this folder is not

This folder is **not**:
- the readiness standard
- the scorecard
- the patch tracker of record
- the repo architecture spec
- proof by itself

The authoritative sources still live elsewhere.

## Actual controlling authority

For institutional readiness and readiness scoring, the controlling docs are:
- `docs/INSTITUTIONAL_READINESS_LOCK.md`
- `docs/INSTITUTIONAL_SCORECARD.md`

For current remaining MAIN-system closure work, use the active remaining-work patch plan and command-center ledger.

For code truth, runtime truth, and proof truth:
- code beats docs
- runtime/proof beats docs
- DB-backed truth beats placeholders
- mounted surface does not automatically mean authoritative truth

## Recommended use order

1. Read `AI_WORKING_RULES.md`
2. Read `MASTER_COMMAND_BRIEF.md`
3. Read `OPERATOR_LEDGER.md`
4. Pick or write the right subsystem brief from `templates/`
5. Fill a patch packet from `templates/PATCH_PACKET_TEMPLATE.md`
6. Use the mapping prompt template before fuzzy tasks
7. Use the execution prompt template after mapping is done
8. Update the ledger after proof, audit, or patch outcomes

## Current repo-specific stance

At the time this pack was created:
- `MAIN` is the only canonical engine
- `EXP` is research-side only and must not widen MAIN readiness or proof burden
- readiness, completion, viability, live-ops, and maintainability must stay separate
- one patch at a time remains the default operating rule
- when patching, return full updated files or the full requested section only

## Folder map

- `AI_WORKING_RULES.md` — standing rules for all AI tools
- `MASTER_COMMAND_BRIEF.md` — compact MiniQuantDesk-wide command brief
- `OPERATOR_LEDGER.md` — living project ledger starter
- `templates/` — reusable prompts and packet templates
- `archive/` — sample/example material only

## Maintenance rule

Do not let this folder become another drifting authority layer.
When readiness, patch status, or repo posture changes, update:
- `MASTER_COMMAND_BRIEF.md`
- `OPERATOR_LEDGER.md`

Leave the templates mostly stable unless the workflow itself changes.
