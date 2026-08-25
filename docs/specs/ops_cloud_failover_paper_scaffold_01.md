# OPS-CLOUD-FAILOVER-PAPER-SCAFFOLD-01

STATUS: `CLOSED_LOCAL_SCAFFOLD_ONLY`

## What this is

A provider-neutral **future connection contract** so a cloud VM could
eventually plug into MiniQuantDesk without designing that interface during
the future emergency/failover implementation. It is committed under
`tools/ops/cloud_failover_scaffold/`:

- `schema.json` -- JSON Schema for the future standby identity/status
  payload.
- `config.example.json` -- example operator config. `enabled` defaults
  `false`.
- `status_payload.example.json` -- example payload fixture (not generated
  by any live process).
- `validate_scaffold.py` -- read-only validator over two static JSON files.
- `test_validate_scaffold.py` -- unit tests (28 cases) covering every
  negative control below.

And guarded statically by `scripts/guards/check_cloud_failover_scaffold_safety.ps1`.

## What this is NOT

This scaffold does **not** implement cloud failover, warm standby, or high
availability. It does not:

- provision or start a cloud VM;
- open a network connection anywhere;
- generate a durable leadership lease or a fencing/generation token;
- promote a standby to primary;
- arm, start, or otherwise mutate runtime state;
- call any `mqk-daemon` route;
- connect to Alpaca or any broker;
- read live git/DB/reconcile state from this repository at all -- the
  validator only ever inspects the two JSON files handed to it on the
  command line.

`standby_readiness: ready_for_future_takeover_evaluation` means only that
this scaffold's own structural checks passed. **It is not permission to
trade, arm, or promote a standby**, and this contract defines no
transition to execution anywhere.

## The payload contract (`schema.json`)

| Field | Purpose |
|---|---|
| `schema_version` | Fixed to `ops-cloud-failover-paper-scaffold-v1`. |
| `node_id` | Free-form node identifier. |
| `node_role` | `primary` \| `standby`. |
| `deployment_mode` | Fixed to `"paper"` -- any other value is `UNSUPPORTED_LIVE_MODE`. |
| `live_capability` | Fixed to `false` -- no transition to `true` exists anywhere in this contract. |
| `git_sha` | This node's observed HEAD sha. |
| `config_identity` | Hash of this node's local layered config. |
| `protocol_schema_identity` | Which version of this schema the payload targets. |
| `database_recovery_snapshot_identity` | Identity of the most recent [OPS-OFFSITE-BACKUP-01](../../scripts/windows/Backup-MiniQuantDeskRecovery.ps1) manifest this node knows about. |
| `research_registry_identity` / `promotion_evidence_identity` | Optional, nullable. |
| `last_verified_backup_snapshot_identity` | Empty = stale/absent. |
| `reconcile_status_summary` | Free-form; validator treats only `"ok"` as passing. |
| `local_runtime_authority_status` | Free-form; validator treats only `"available"` as passing. |
| `future_lease_backend` | Placeholder for a FUTURE durable renewable-leadership lease backend identifier. Empty = not configured; this scaffold never acquires one. |
| `future_fencing_generation` | Placeholder for a FUTURE fencing/generation token. This scaffold never generates one. |
| `standby_readiness` | Closed enum: `not_configured` \| `identity_mismatch` \| `backup_stale` \| `reconcile_required` \| `authority_unavailable` \| `ready_for_future_takeover_evaluation`. |

## The config scaffold (`config.example.json`)

```json
{
  "enabled": false,
  "provider": "",
  "standby_endpoint": "",
  "node_id": "",
  "expected_git_sha": "",
  "lease_backend": "",
  "deployment_mode": "paper",
  "live_capability_requested": false
}
```

`enabled` MUST default `false`. No credential/token/API-key field exists
in this config shape at all -- there is nothing to leak.

## Validator verdicts (`validate_scaffold.py`)

A closed, exit-code-bearing vocabulary:

| Verdict | Meaning | Exit |
|---|---|---|
| `MALFORMED_CONFIG` | Config/payload file missing, invalid JSON, or fails the required-field/enum shape check. | 1 |
| `NOT_CONFIGURED` | `config.enabled` is `false` (the shipped default). | 1 |
| `UNSUPPORTED_LIVE_MODE` | `deployment_mode` isn't `"paper"`, or any live-capability flag isn't `false`. | 1 |
| `MISSING_FENCING_BACKEND` | `config.lease_backend` is empty -- a payload can never be `ready_for_future_takeover_evaluation` without one. | 1 |
| `IDENTITY_MISMATCH` | `config.expected_git_sha` is set and disagrees with `payload.git_sha`. | 1 |
| `SCAFFOLD_VALID` | Every check above passed. Structural/contract verdict only. | 0 |

`SCAFFOLD_VALID` additionally reports a `granular_standby_readiness` value
(the payload's own more granular `backup_stale` / `reconcile_required` /
`authority_unavailable` / `ready_for_future_takeover_evaluation`
classification) -- always labeled as not constituting execution
authority.

## Negative controls (proven in `test_validate_scaffold.py`)

- **E1** `enabled` defaults false -> `NOT_CONFIGURED`.
- **E2** Live configuration (`deployment_mode`, `live_capability`,
  `live_capability_requested`) is rejected -> `UNSUPPORTED_LIVE_MODE`.
- **E3** Missing fencing backend can never report future-takeover-ready ->
  `MISSING_FENCING_BACKEND`.
- **E4** Identity mismatch is visible -> `IDENTITY_MISMATCH`; a blank
  `expected_git_sha` never falsely triggers one.
- **E5** Malformed config/payload (missing file, invalid JSON, missing
  required field, wrong `schema_version`, an out-of-enum
  `standby_readiness`) fails closed -> `MALFORMED_CONFIG`, never a crash.
- **E6/E7** Static source-fence tests prove the validator module imports no
  networking/subprocess/DB capability and contains no daemon-route/
  Alpaca/launcher reference.
- **E8** The committed example config carries no secret-shaped key, and
  every non-fixed string value ships empty.
- **E9** A `standby_endpoint` value alone, with no `lease_backend`, still
  reports `MISSING_FENCING_BACKEND` -- an endpoint alone can never
  constitute standby authority.

## Future work that remains explicit (NOT satisfied by this scaffold)

A real `OPS-CLOUD-FAILOVER-PAPER-01` program still requires a separate,
future, dedicated implementation covering:

- a durable, renewable leadership lease;
- a fencing/generation token and stale-primary fencing;
- network-partition fail-closed behavior;
- split-brain prevention;
- read-only broker reconciliation before any takeover;
- order/position/account reconciliation across nodes;
- source/config/protocol identity match enforcement at takeover time;
- handling a stale primary that returns after a cloud takeover;
- safe handback from standby to primary;
- crash-before/after-broker-ACK tests;
- simultaneous-start tests;
- an at-most-one-execution-authority proof.

This scaffold satisfies **none** of those execution-authority
requirements. It only gives a future implementation a starting contract to
connect to.
