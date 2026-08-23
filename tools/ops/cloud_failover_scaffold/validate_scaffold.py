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
    MISSING_FENCING_BACKEND       -- config.lease_backend or
                                    payload.future_lease_backend is empty, they
                                    do not match, or payload.future_fencing_generation
                                    is not a present non-negative integer. A
                                    payload can NEVER report
                                    ready_for_future_takeover_evaluation
                                    without a real fencing/lease identity.
    IDENTITY_MISMATCH             -- config.node_id/payload.node_id are blank
                                    or do not match, config.expected_git_sha or
                                    payload.git_sha is blank, or
                                    expected_git_sha != payload.git_sha.
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

NODE_ROLE_VALUES = {"primary", "standby"}

# Accepted scaffold protocol/schema identity a payload must declare itself
# against. Reuses SCHEMA_VERSION rather than a second parallel constant --
# there is exactly one accepted contract identity in this scaffold.
ACCEPTED_PROTOCOL_SCHEMA_IDENTITY = SCHEMA_VERSION

# Human-readable annotation key tolerated in committed example/fixture files
# (see config.example.json / status_payload.example.json). Never part of the
# functional contract and never read by any check below.
ALWAYS_ALLOWED_METADATA_KEYS = {"_comment"}


def _is_str(value: Any) -> bool:
    return isinstance(value, str)


def _is_bool(value: Any) -> bool:
    return isinstance(value, bool)


def _is_str_or_none(value: Any) -> bool:
    return value is None or isinstance(value, str)


def _is_nonnegative_int_or_none(value: Any) -> bool:
    # bool is a subclass of int in Python -- explicitly excluded so
    # True/False can never silently pass as a fencing-generation integer.
    if value is None:
        return True
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


# Explicit type contract per field -- checked before ANY semantic/truthiness
# check runs, so a wrong-shaped value (e.g. enabled="true", live_capability=0,
# future_fencing_generation="1") fails closed as MALFORMED_CONFIG rather than
# silently coercing through a Python truthiness check further down.
CONFIG_FIELD_TYPES: dict[str, Any] = {
    "enabled": _is_bool,
    "provider": _is_str,
    "standby_endpoint": _is_str,
    "node_id": _is_str,
    "expected_git_sha": _is_str,
    "lease_backend": _is_str,
    "deployment_mode": _is_str,
    "live_capability_requested": _is_bool,
}

PAYLOAD_FIELD_TYPES: dict[str, Any] = {
    "schema_version": _is_str,
    "node_id": _is_str,
    "node_role": _is_str,
    "deployment_mode": _is_str,
    "live_capability": _is_bool,
    "git_sha": _is_str,
    "config_identity": _is_str,
    "protocol_schema_identity": _is_str,
    "database_recovery_snapshot_identity": _is_str,
    "research_registry_identity": _is_str_or_none,
    "promotion_evidence_identity": _is_str_or_none,
    "last_verified_backup_snapshot_identity": _is_str,
    "reconcile_status_summary": _is_str,
    "local_runtime_authority_status": _is_str,
    "future_lease_backend": _is_str,
    "future_fencing_generation": _is_nonnegative_int_or_none,
    "standby_readiness": _is_str,
}

ALL_CONFIG_KEYS = set(CONFIG_FIELD_TYPES) | ALWAYS_ALLOWED_METADATA_KEYS
ALL_PAYLOAD_KEYS = set(PAYLOAD_FIELD_TYPES) | ALWAYS_ALLOWED_METADATA_KEYS


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


def _fields_well_typed(data: dict[str, Any], type_map: dict[str, Any]) -> bool:
    """Type-checks only fields actually present -- required-field presence is
    validated separately by _has_required_fields, so an absent optional field
    is not a type failure here."""
    for field, checker in type_map.items():
        if field not in data:
            continue
        if not checker(data[field]):
            return False
    return True


def _no_unknown_keys(data: dict[str, Any], allowed_keys: set[str]) -> bool:
    return all(key in allowed_keys for key in data.keys())


def compute_granular_standby_readiness(payload: dict[str, Any]) -> str:
    """Auxiliary, more granular readiness classification carried inside the
    payload's own `standby_readiness` field. Distinct from (and narrower
    than) the validator's top-level SCAFFOLD_VALID verdict -- this never
    overrides a top-level MALFORMED_CONFIG/UNSUPPORTED_LIVE_MODE/
    MISSING_FENCING_BACKEND/IDENTITY_MISMATCH verdict; it is only computed
    when the top-level verdict is already SCAFFOLD_VALID.

    A structurally missing/invalid future_fencing_generation is re-checked
    here too (defense in depth): by the time SCAFFOLD_VALID is reached the
    top-level MISSING_FENCING_BACKEND check has already enforced this, but
    "ready_for_future_takeover_evaluation" must never be reachable through
    this function alone if that invariant is ever weakened above.
    """
    if not payload.get("last_verified_backup_snapshot_identity"):
        return "backup_stale"
    if payload.get("reconcile_status_summary") != "ok":
        return "reconcile_required"
    if payload.get("local_runtime_authority_status") != "available":
        return "authority_unavailable"
    if not _is_nonnegative_int_or_none(payload.get("future_fencing_generation")) or payload.get("future_fencing_generation") is None:
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

    # Type contract (D-R1-style fail-closed: wrong-shaped values, e.g.
    # enabled="true"/live_capability=0/future_fencing_generation="1", must
    # never pass via Python truthiness) and closed key sets, both checked
    # before any semantic value is trusted.
    if not _no_unknown_keys(config, ALL_CONFIG_KEYS):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "config has an unexpected/unknown field"}
    if not _no_unknown_keys(payload, ALL_PAYLOAD_KEYS):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload has an unexpected/unknown field"}
    if not _fields_well_typed(config, CONFIG_FIELD_TYPES):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "config has a field with the wrong type"}
    if not _fields_well_typed(payload, PAYLOAD_FIELD_TYPES):
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload has a field with the wrong type"}

    if payload.get("schema_version") != SCHEMA_VERSION:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload schema_version mismatch"}
    if payload.get("protocol_schema_identity") != ACCEPTED_PROTOCOL_SCHEMA_IDENTITY:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload protocol_schema_identity does not match the accepted scaffold contract identity"}
    if payload.get("standby_readiness") not in STANDBY_READINESS_VALUES:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload standby_readiness not in the closed enum set"}
    if payload.get("node_role") not in NODE_ROLE_VALUES:
        return {"verdict": ScaffoldVerdict.MALFORMED_CONFIG, "reason": "payload node_role not in the closed enum set (primary|standby)"}

    if config.get("enabled") is not True:
        return {"verdict": ScaffoldVerdict.NOT_CONFIGURED, "reason": "config.enabled is false (default)"}

    if payload.get("deployment_mode") != "paper" or config.get("deployment_mode") != "paper":
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "deployment_mode is not paper"}
    if payload.get("live_capability") is not False:
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "payload.live_capability is not false"}
    if config.get("live_capability_requested") is not False:
        return {"verdict": ScaffoldVerdict.UNSUPPORTED_LIVE_MODE, "reason": "config.live_capability_requested is not false"}

    config_lease_backend = config.get("lease_backend") or ""
    payload_lease_backend = payload.get("future_lease_backend") or ""
    fencing_generation = payload.get("future_fencing_generation")
    if not config_lease_backend:
        return {"verdict": ScaffoldVerdict.MISSING_FENCING_BACKEND, "reason": "config.lease_backend is empty"}
    if not payload_lease_backend:
        return {"verdict": ScaffoldVerdict.MISSING_FENCING_BACKEND, "reason": "payload.future_lease_backend is empty"}
    if payload_lease_backend != config_lease_backend:
        return {
            "verdict": ScaffoldVerdict.MISSING_FENCING_BACKEND,
            "reason": f"payload.future_lease_backend={payload_lease_backend!r} != config.lease_backend={config_lease_backend!r}",
        }
    if fencing_generation is None:
        return {"verdict": ScaffoldVerdict.MISSING_FENCING_BACKEND, "reason": "payload.future_fencing_generation is null -- no usable fencing-generation claim"}

    config_node_id = config.get("node_id") or ""
    payload_node_id = payload.get("node_id") or ""
    if not config_node_id:
        return {"verdict": ScaffoldVerdict.IDENTITY_MISMATCH, "reason": "config.node_id is empty"}
    if not payload_node_id:
        return {"verdict": ScaffoldVerdict.IDENTITY_MISMATCH, "reason": "payload.node_id is empty"}
    if config_node_id != payload_node_id:
        return {
            "verdict": ScaffoldVerdict.IDENTITY_MISMATCH,
            "reason": f"config.node_id={config_node_id!r} != payload.node_id={payload_node_id!r}",
        }

    expected_sha = config.get("expected_git_sha") or ""
    actual_sha = payload.get("git_sha") or ""
    if not expected_sha:
        return {"verdict": ScaffoldVerdict.IDENTITY_MISMATCH, "reason": "config.expected_git_sha is empty in an enabled config"}
    if not actual_sha:
        return {"verdict": ScaffoldVerdict.IDENTITY_MISMATCH, "reason": "payload.git_sha is empty"}
    if expected_sha != actual_sha:
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
