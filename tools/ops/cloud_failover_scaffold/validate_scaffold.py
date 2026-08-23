#!/usr/bin/env python3
"""OPS-CLOUD-FAILOVER-PAPER-SCAFFOLD-01: read-only scaffold validator.

SCAFFOLD ONLY. This tool validates two static JSON files (a config and a
standby status payload) against the future-standby-connection contract
documented in docs/specs/ops_cloud_failover_paper_scaffold_01.md. It does
not implement cloud failover, does not generate a live payload, and does
not integrate with the daemon/runtime in any way.

This script MUST NEVER: open a network connection, spawn a subprocess,
touch a database, read/write repository production code, acquire a lease,
generate a fencing token, promote a standby, arm anything, or call any
mqk-daemon route. It reads exactly the two files passed to it (plus this
module's own embedded schema shape) and prints a verdict. Nothing else.

Verdict vocabulary (top-level, exit-code bearing):
    MALFORMED_CONFIG            -- config or payload JSON is missing/invalid/
                                    fails the required-field shape check.
    NOT_CONFIGURED               -- config.enabled is false (the default).
    UNSUPPORTED_LIVE_MODE         -- deployment_mode != "paper", or
                                    live_capability/live_capability_requested
                                    is anything but false.
    MISSING_FENCING_BACKEND       -- config.lease_backend is empty. A payload
                                    can NEVER report
                                    ready_for_future_takeover_evaluation
                                    without a fencing backend configured.
    IDENTITY_MISMATCH             -- config.expected_git_sha is set and does
                                    not match payload.git_sha.
    SCAFFOLD_VALID                -- every check above passed. This is a
                                    STRUCTURAL/CONTRACT verdict only -- it is
                                    never permission to trade, arm, or
                                    promote a standby.

Usage:
    python validate_scaffold.py --config config.example.json --payload status_payload.example.json
    python validate_scaffold.py --config config.example.json --payload status_payload.example.json --json

Exit codes: 0 = SCAFFOLD_VALID, 1 = any other verdict (including a refused/
malformed input -- that is a safety refusal, not a crash).
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

SCHEMA_VERSION = "ops-cloud-failover-paper-scaffold-v1"

REQUIRED_PAYLOAD_FIELDS = (
    "schema_version",
    "node_id",
    "node_role",
    "deployment_mode",
    "live_capability",
    "git_sha",
    "config_identity",
    "protocol_schema_identity",
    "database_recovery_snapshot_identity",
    "last_verified_backup_snapshot_identity",
    "reconcile_status_summary",
    "local_runtime_authority_status",
    "future_lease_backend",
    "future_fencing_generation",
    "standby_readiness",
)

REQUIRED_CONFIG_FIELDS = (
    "enabled",
    "provider",
    "standby_endpoint",
    "node_id",
    "expected_git_sha",
    "lease_backend",
    "deployment_mode",
    "live_capability_requested",
)

STANDBY_READINESS_VALUES = {
    "not_configured",
    "identity_mismatch",
    "backup_stale",
    "reconcile_required",
    "authority_unavailable",
    "ready_for_future_takeover_evaluation",
}


class ScaffoldVerdict:
    MALFORMED_CONFIG = "MALFORMED_CONFIG"
    NOT_CONFIGURED = "NOT_CONFIGURED"
    UNSUPPORTED_LIVE_MODE = "UNSUPPORTED_LIVE_MODE"
    MISSING_FENCING_BACKEND = "MISSING_FENCING_BACKEND"
    IDENTITY_MISMATCH = "IDENTITY_MISMATCH"
    SCAFFOLD_VALID = "SCAFFOLD_VALID"


def _load_json(path: str) -> dict[str, Any] | None:
    """Read-only local file load. Returns None on any parse/read failure --
    never raises past this boundary, so a malformed file is always a
    reported verdict, never an uncaught exception."""
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    return data


def _has_required_fields(data: dict[str, Any], required: tuple[str, ...]) -> bool:
    return all(field in data for field in required)


def compute_granular_standby_readiness(payload: dict[str, Any]) -> str:
    """Auxiliary, more granular readiness classification carried inside the
    payload's own `standby_readiness` field. Distinct from (and narrower
    than) the validator's top-level SCAFFOLD_VALID verdict -- this never
    overrides a top-level MALFORMED_CONFIG/UNSUPPORTED_LIVE_MODE/
    MISSING_FENCING_BACKEND/IDENTITY_MISMATCH verdict; it is only computed
    when the top-level verdict is already SCAFFOLD_VALID.
    """
    if not payload.get("last_verified_backup_snapshot_identity"):
        return "backup_stale"
    if payload.get("reconcile_status_summary") != "ok":
        return "reconcile_required"
    if payload.get("local_runtime_authority_status") != "available":
        return "authority_unavailable"
    return "ready_for_future_takeover_evaluation"


def validate(config: dict[str, Any] | None, payload: dict[str, Any] | None) -> dict[str, Any]:
    """Pure function: two dicts in, a verdict dict out. No I/O of any kind."""

    if config is None or payload is None:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "config or payload file missing/invalid JSON"}

    if not _has_required_fields(config, REQUIRED_CONFIG_FIELDS):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "config missing required field(s)"}
    if not _has_required_fields(payload, REQUIRED_PAYLOAD_FIELDS):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload missing required field(s)"}
    if payload.get("schema_version") != SCHEMA_VERSION:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload schema_version mismatch"}
    if payload.get("standby_readiness") not in STANDBY_READINESS_VALUES:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload standby_readiness not in the closed enum set"}

    if config.get("enabled") is not True:
        return {"verdict": ScaffoldVerdict.NOT_CONFIGURED, "reason": "config.enabled is false (default)"}

    if payload.get("deployment_mode") != "paper" or config.get("deployment_mode") != "paper":
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "deployment_mode is not paper"}
    if payload.get("live_capability") is not False:
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "payload.live_capability is not false"}
    if config.get("live_capability_requested") is not False:
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "config.live_capability_requested is not false"}

    if not config.get("lease_backend"):
        return {"verdict": ScaffoldVerdict.MISSING_FENCING_BACKEND, "reason": "config.lease_backend is empty"}

    expected_sha = config.get("expected_git_sha") or ""
    actual_sha = payload.get("git_sha") or ""
    if expected_sha and actual_sha and expected_sha != actual_sha:
        return {
            "verdict": ScaffoldVerdict.IDENTITY_MISMATCH,
            "reason": f"expected_git_sha={expected_sha!r} != payload.git_sha={actual_sha!r}",
        }

    return {
        "verdict": ScaffoldVerdict.SCAFFOLD_VALID,
        "reason": "all scaffold-contract checks passed (structural only -- not execution authority)",
        "granular_standby_readiness": compute_granular_standby_readiness(payload),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="OPS-CLOUD-FAILOVER-PAPER-SCAFFOLD-01 read-only validator")
    parser.add_argument("--config", required=True, help="Path to a scaffold config JSON file")
    parser.add_argument("--payload", required=True, help="Path to a standby status payload JSON file")
    parser.add_argument("--json", action="store_true", help="Print the full verdict as JSON")
    args = parser.parse_args(argv)

    config = _load_json(args.config)
    payload = _load_json(args.payload)
    result = validate(config, payload)

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(f"verdict={result['verdict']} reason={result['reason']}")
        if "granular_standby_readiness" in result:
            print(f"granular_standby_readiness={result['granular_standby_readiness']} (NOT permission to trade)")

    return 0 if result["verdict"] == ScaffoldVerdict.SCAFFOLD_VALID else 1


if __name__ == "__main__":
    sys.exit(main())
