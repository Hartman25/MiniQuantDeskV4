# Subsystem Brief Template — MiniQuantDesk V4

Make one of these per subsystem. Keep it narrow and factual.

Suggested examples:
- `docs/briefs/broker_inbound.md`
- `docs/briefs/daemon_truth.md`
- `docs/briefs/gui_truth.md`
- `docs/briefs/runtime_orchestration.md`
- `docs/briefs/market_data_ingest.md`
- `docs/briefs/proof_harness.md`
- `docs/briefs/research_pipeline.md`

---

## Subsystem name
- **Name:**
- **Domain:**
- **Why this subsystem exists:**

## Scope boundary

### What this subsystem owns
- 
- 
- 

### What this subsystem does not own
- 
- 
- 

## Authoritative files
List the files the model should inspect before speaking confidently.

- **Path:**
  - **Role:**
  - **Why authoritative:**

Repeat as needed.

## Adjacent files often mistaken as authority
List helpful but secondary files.

- **Path:**
  - **Role:**
  - **Why secondary only:**

## Key contracts
Examples: routes, DB tables, trait boundaries, manifest structures, scenario tests, proof commands.

- **Contract / route / table / type / test:**
  - **Meaning:**
  - **Failure mode if misunderstood:**

## Key invariants
- 
- 
- 

## Runtime / DB / operator truth hooks
If this subsystem depends on runtime semantics, DB state, migrations, scenario tests, or operator procedures, list them.

- **Item:**
  - **Relevance:**
  - **Authoritative source:**

## Important tests and proof
- **Test file / command:**
  - **What it proves:**
  - **What it does not prove:**

Repeat as needed.

## Common traps in this subsystem
- 
- 
- 

## Open questions / ambiguity
- 
- 
- 

## Minimal file bundle for a serious task here
- 
- 
- 

## MiniQuantDesk examples to keep in mind
Use the section below only if relevant.

- mounted truth surface vs authoritative backend truth
- DB-backed truth required by readiness lock
- GUI rendering of unavailable vs authoritative empty
- restart/mode-transition semantics vs surface placeholders
- research-side artifact semantics vs canonical production truth
- EXP isolation from MAIN
