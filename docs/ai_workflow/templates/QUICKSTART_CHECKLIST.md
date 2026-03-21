# Quickstart Checklist — MiniQuantDesk V4

Use this when setting up a serious AI-assisted task.

## One-time setup
- [ ] Read `docs/ai_workflow/AI_WORKING_RULES.md`
- [ ] Update `docs/ai_workflow/MASTER_COMMAND_BRIEF.md` if repo posture changed
- [ ] Update `docs/ai_workflow/OPERATOR_LEDGER.md`
- [ ] Confirm the controlling readiness docs are current
- [ ] Confirm the remaining-work patch list is current

## Before each serious AI prompt
- [ ] Identify the active audit axis or patch objective
- [ ] Decide whether the task is MAIN or EXP
- [ ] Pick or write the subsystem brief
- [ ] Fill out a patch packet if patching or doing targeted review
- [ ] Assemble the smallest useful file bundle
- [ ] Include relevant proof output or failing logs
- [ ] Ban scope drift explicitly

## Best first subsystem briefs
- [ ] broker / execution lifecycle
- [ ] daemon truth surfaces
- [ ] GUI truth rendering
- [ ] market data / ingestion
- [ ] proof harness / scenario tests
- [ ] DB schema / migrations
- [ ] controls / halt / recovery
- [ ] research / artifacts / promotion

## After each serious AI result
- [ ] Verify whether the answer stayed in scope
- [ ] Verify that claims match the files actually inspected
- [ ] Run proof / tests
- [ ] Update the operator ledger
- [ ] Do not mark closure unless both proof and success criteria are satisfied
- [ ] If the task touched EXP, verify MAIN proof burden did not widen
