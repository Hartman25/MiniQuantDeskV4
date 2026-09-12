---
name: mqd-external-research
description: Use external documentation, provider docs, or papers without letting them override MiniQuantDesk's actual behavior. Use when a library/broker/provider API question or a statistical methodology question needs primary-source evidence.
---

# mqd-external-research

External sources establish what a system **should** mean. This repository's
code/tests/data establish what MiniQuantDesk **actually** does. When they
disagree, preserve the contradiction — do not silently reconcile it
(CLAUDE.md §18).

## Evidence order

1. Repository code/tests/data
2. Local read-only evidence (DB, fixtures)
3. Version-matched official docs/source
4. Primary papers / provider documentation
5. High-quality secondary evidence, only when necessary

Use the least expensive tier that resolves the question. Prefer Context7
for version-specific library/API docs, official provider docs for
broker/data-provider semantics, and primary papers for methodology.
Community claims (forum posts, blogs, Reddit) are not statistical or API
proof.

Do not re-research a methodology that is already verified and frozen
(CLAUDE.md §19) during unrelated work.

## Procedure

1. State the question and which evidence tier it needs.
2. Fetch the version-matched or primary source. Cite it.
3. Compare against the current repository implementation for the same
   behavior.
4. If they agree: state the confirmed behavior and cite both.
5. If they disagree: report `EXTERNAL = X`, `LOCAL = Y`, and stop — do not
   change local behavior and do not claim the docs prove local behavior.
   The contradiction is the finding; only the current mission/controller
   can authorize resolving it.

## Artifact behavior

- A temporary lookup (a single API fact, a version quirk): report the
  result in conversation. No automatic Markdown file.
- A durable architectural/statistical decision: update the existing
  canonical audit/runbook/ADR only if the current mission authorizes it.
- A new artifact: only when the current mission justifies one.

## Hard stops

- Never claim external documentation overrides or proves current
  repository behavior.
- Never silently make repository code agree with external docs — that
  requires mission authorization.
- Do not treat secondary/community sources as load-bearing for trading
  methodology.
- Do not invoke another skill; hand back to the mission/controller.
