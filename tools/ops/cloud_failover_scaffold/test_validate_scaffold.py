"""Proof for validate_scaffold.py (OPS-CLOUD-FAILOVER-PAPER-SCAFFOLD-01).

Pure in-memory unit tests against `validate()` (no file I/O in most cases)
plus a handful of static source-text assertions proving the validator
module itself never imports a networking/subprocess capability. Run with:

    python -m unittest tools/ops/cloud_failover_scaffold/test_validate_scaffold.py -v

or, from this directory:

    python -m unittest test_validate_scaffold -v
"""

from __future__ import annotations

import copy
import json
import os
import unittest

import validate_scaffold as vs

THIS_DIR = os.path.dirname(os.path.abspath(__file__))


def base_config(**overrides) -> dict:
    # Defaults represent a scaffold that WOULD be structurally valid if
    # enabled -- node_id/lease_backend/expected_git_sha are all nonblank and
    # consistent with base_payload()'s own defaults below, so an individual
    # test only needs to override the one field it's actually exercising
    # (E-R1 repair: node_id/lease_backend/git_sha blankness are now each
    # independently gating, so a shared "everything blank" baseline would
    # make every enabled-config test collide on the same early verdict).
    cfg = {
        "enabled": False,
        "provider": "",
        "standby_endpoint": "",
        "node_id": "node-1",
        "expected_git_sha": "a" * 40,
        "lease_backend": "future-lease-x",
        "deployment_mode": "paper",
        "live_capability_requested": False,
    }
    cfg.update(overrides)
    return cfg


def base_payload(**overrides) -> dict:
    payload = {
        "schema_version": vs.SCHEMA_VERSION,
        "node_id": "node-1",
        "node_role": "standby",
        "deployment_mode": "paper",
        "live_capability": False,
        "git_sha": "a" * 40,
        "config_identity": "",
        "protocol_schema_identity": vs.SCHEMA_VERSION,
        "database_recovery_snapshot_identity": "",
        "research_registry_identity": None,
        "promotion_evidence_identity": None,
        "last_verified_backup_snapshot_identity": "",
        "reconcile_status_summary": "unavailable",
        "local_runtime_authority_status": "unavailable",
        "future_lease_backend": "future-lease-x",
        "future_fencing_generation": 1,
        "standby_readiness": "not_configured",
    }
    payload.update(overrides)
    return payload


class ScaffoldValidatorTests(unittest.TestCase):
    # --- E1: enabled defaults false -----------------------------------
    def test_e1_disabled_config_is_not_configured(self):
        result = vs.validate(base_config(), base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.NOT_CONFIGURED)

    # --- E2: Live configuration rejected --------------------------------
    def test_e2_non_paper_deployment_mode_is_rejected(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x", deployment_mode="live-capital")
        payload = base_payload(deployment_mode="live-capital")
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.UNSUPPORTED_LIVE_MODE)

    def test_e2_live_capability_true_is_rejected(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x")
        payload = base_payload(live_capability=True)
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.UNSUPPORTED_LIVE_MODE)

    def test_e2_live_capability_requested_true_is_rejected(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x", live_capability_requested=True)
        result = vs.validate(cfg, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.UNSUPPORTED_LIVE_MODE)

    # --- E3 / E9: missing fencing backend can never be ready; an
    #     endpoint alone is never sufficient authority --------------------
    def test_e3_missing_lease_backend_blocks_readiness(self):
        cfg = base_config(enabled=True, lease_backend="")
        result = vs.validate(cfg, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MISSING_FENCING_BACKEND)

    def test_e9_standby_endpoint_alone_is_never_authority(self):
        # standby_endpoint is set to a real-looking value, but lease_backend
        # is still empty -- must still refuse, proving the endpoint field
        # alone cannot constitute standby authority.
        cfg = base_config(enabled=True, standby_endpoint="https://standby.example.internal:9443", lease_backend="")
        result = vs.validate(cfg, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MISSING_FENCING_BACKEND)
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)

    # --- E4: identity mismatch visible -----------------------------------
    def test_e4_identity_mismatch_detected(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x", expected_git_sha="a" * 40)
        payload = base_payload(git_sha="b" * 40)
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)

    def test_e4_matching_identity_is_not_a_mismatch(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x", expected_git_sha="a" * 40)
        payload = base_payload(git_sha="a" * 40)
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)

    def test_e4_blank_expected_sha_now_blocks(self):
        # E-R1 repair: expected_git_sha="" in an ENABLED config used to be
        # treated as "no claim to check" and fall through to SCAFFOLD_VALID
        # -- one of the exact independent-review false positives. An enabled
        # config must declare a real expected_git_sha; a blank one now fails
        # closed instead of silently skipping identity verification.
        cfg = base_config(enabled=True, expected_git_sha="")
        payload = base_payload(git_sha="c" * 40)
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)

    # --- E5: malformed config fails closed --------------------------------
    def test_e5_none_config_is_malformed(self):
        result = vs.validate(None, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_none_payload_is_malformed(self):
        result = vs.validate(base_config(), None)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_config_missing_required_field_is_malformed(self):
        cfg = base_config()
        del cfg["lease_backend"]
        result = vs.validate(cfg, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_payload_missing_required_field_is_malformed(self):
        payload = base_payload()
        del payload["standby_readiness"]
        result = vs.validate(base_config(), payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_payload_wrong_schema_version_is_malformed(self):
        payload = base_payload(schema_version="some-other-version")
        result = vs.validate(base_config(), payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_payload_standby_readiness_outside_enum_is_malformed(self):
        payload = base_payload(standby_readiness="definitely_ready_go_trade")
        result = vs.validate(base_config(), payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_e5_load_json_missing_file_returns_none_not_an_exception(self):
        # Proves _load_json fails closed (None) rather than raising past its
        # boundary for a nonexistent file.
        self.assertIsNone(vs._load_json(os.path.join(THIS_DIR, "does_not_exist.json")))

    def test_e5_load_json_invalid_json_returns_none(self):
        bad_path = os.path.join(THIS_DIR, "_scratch_invalid.json")
        with open(bad_path, "w", encoding="utf-8") as f:
            f.write("{ not valid json ")
        try:
            self.assertIsNone(vs._load_json(bad_path))
        finally:
            os.remove(bad_path)

    # --- Positive control + granular readiness ----------------------------
    def test_positive_full_valid_scaffold_reports_scaffold_valid(self):
        cfg = base_config(enabled=True, lease_backend="future-lease-x", expected_git_sha="a" * 40)
        payload = base_payload(
            git_sha="a" * 40,
            last_verified_backup_snapshot_identity="sha256:deadbeef",
            reconcile_status_summary="ok",
            local_runtime_authority_status="available",
            standby_readiness="ready_for_future_takeover_evaluation",
        )
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["granular_standby_readiness"], "ready_for_future_takeover_evaluation")

    def test_granular_readiness_backup_stale_when_no_snapshot_identity(self):
        payload = base_payload(last_verified_backup_snapshot_identity="")
        self.assertEqual(vs.compute_granular_standby_readiness(payload), "backup_stale")

    def test_granular_readiness_reconcile_required_when_not_ok(self):
        payload = base_payload(last_verified_backup_snapshot_identity="x", reconcile_status_summary="dirty")
        self.assertEqual(vs.compute_granular_standby_readiness(payload), "reconcile_required")

    def test_granular_readiness_authority_unavailable(self):
        payload = base_payload(
            last_verified_backup_snapshot_identity="x",
            reconcile_status_summary="ok",
            local_runtime_authority_status="unavailable",
        )
        self.assertEqual(vs.compute_granular_standby_readiness(payload), "authority_unavailable")

    def test_granular_readiness_ready_when_all_pass(self):
        payload = base_payload(
            last_verified_backup_snapshot_identity="x",
            reconcile_status_summary="ok",
            local_runtime_authority_status="available",
        )
        self.assertEqual(vs.compute_granular_standby_readiness(payload), "ready_for_future_takeover_evaluation")

    # --- ER1-ER5: exact independent-review false-positive mutation tests --
    # Each mutates exactly ONE field away from an otherwise-valid enabled
    # scaffold and proves SCAFFOLD_VALID (and therefore
    # ready_for_future_takeover_evaluation) is unreachable.
    def test_er1_blank_payload_git_sha_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(git_sha=""))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)
        self.assertNotIn("granular_standby_readiness", result)

    def test_er2_blank_payload_future_lease_backend_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(future_lease_backend=""))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MISSING_FENCING_BACKEND)
        self.assertNotIn("granular_standby_readiness", result)

    def test_er3_null_fencing_generation_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(future_fencing_generation=None))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MISSING_FENCING_BACKEND)
        self.assertNotIn("granular_standby_readiness", result)

    def test_er4_invalid_node_role_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(node_role="banana"))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)
        self.assertNotIn("granular_standby_readiness", result)

    def test_er5_invalid_protocol_schema_identity_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(protocol_schema_identity="other"))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)
        self.assertNotIn("granular_standby_readiness", result)

    # --- type confusion: malformed field TYPES must fail closed, never pass
    #     via Python truthiness ---------------------------------------------
    def test_type_confusion_enabled_as_string_is_refused(self):
        result = vs.validate(base_config(enabled="true"), base_payload())
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_type_confusion_live_capability_as_zero_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(live_capability=0))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_type_confusion_fencing_generation_as_string_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(future_fencing_generation="1"))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_type_confusion_fencing_generation_negative_int_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(future_fencing_generation=-1))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_type_confusion_fencing_generation_bool_is_refused(self):
        # bool is a subclass of int in Python -- True/False must never
        # silently pass as a fencing-generation integer.
        result = vs.validate(base_config(enabled=True), base_payload(future_fencing_generation=True))
        self.assertNotEqual(result["verdict"], vs.ScaffoldVerdict.SCAFFOLD_VALID)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    # --- Other E-R1 checks: node_id identity binding, lease-backend match,
    #     unknown-field rejection --------------------------------------------
    def test_config_node_id_blank_in_enabled_config_is_refused(self):
        result = vs.validate(base_config(enabled=True, node_id=""), base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)

    def test_payload_node_id_blank_is_refused(self):
        result = vs.validate(base_config(enabled=True), base_payload(node_id=""))
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)

    def test_node_id_mismatch_is_refused(self):
        result = vs.validate(base_config(enabled=True, node_id="node-1"), base_payload(node_id="node-2"))
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.IDENTITY_MISMATCH)

    def test_future_lease_backend_mismatch_is_refused(self):
        result = vs.validate(
            base_config(enabled=True, lease_backend="future-lease-x"),
            base_payload(future_lease_backend="future-lease-y"),
        )
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MISSING_FENCING_BACKEND)

    def test_config_unknown_field_is_malformed(self):
        cfg = base_config()
        cfg["unexpected_extra_field"] = "x"
        result = vs.validate(cfg, base_payload())
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_payload_unknown_field_is_malformed(self):
        payload = base_payload()
        payload["unexpected_extra_field"] = "x"
        result = vs.validate(base_config(), payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.MALFORMED_CONFIG)

    def test_comment_metadata_key_is_tolerated(self):
        # _comment is the one always-allowed non-functional annotation key
        # (used by the committed example fixtures) -- it must not itself
        # trigger the unknown-field rejection.
        cfg = base_config()
        cfg["_comment"] = "explanatory text"
        payload = base_payload()
        payload["_comment"] = "explanatory text"
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.NOT_CONFIGURED)

    # --- Real example fixtures round-trip ----------------------------------
    def test_real_example_files_are_not_configured_as_shipped(self):
        cfg = vs._load_json(os.path.join(THIS_DIR, "config.example.json"))
        payload = vs._load_json(os.path.join(THIS_DIR, "status_payload.example.json"))
        self.assertIsNotNone(cfg)
        self.assertIsNotNone(payload)
        result = vs.validate(cfg, payload)
        self.assertEqual(result["verdict"], vs.ScaffoldVerdict.NOT_CONFIGURED)

    def test_e8_example_config_has_no_secret_shaped_values(self):
        cfg = vs._load_json(os.path.join(THIS_DIR, "config.example.json"))
        secret_key_fragments = ("key", "secret", "token", "password", "credential")
        for key, value in cfg.items():
            if not isinstance(value, str):
                continue
            lowered_key = key.lower()
            if any(frag in lowered_key for frag in secret_key_fragments):
                self.fail(f"config.example.json has a secret-shaped key '{key}' -- must not exist in a committed scaffold example")
            # Every string value in the shipped example is empty or a fixed
            # non-secret constant (deployment_mode="paper", and the
            # human-readable _comment field).
            if key not in ("deployment_mode", "_comment"):
                self.assertEqual(value, "", f"'{key}' must be an empty placeholder in the committed example, got {value!r}")


class ScaffoldStaticSourceFenceTests(unittest.TestCase):
    """E6/E7: static proof that the validator module cannot start a
    runtime, acquire a lease, or talk to a broker/daemon -- it simply never
    imports anything capable of it."""

    @classmethod
    def setUpClass(cls):
        with open(os.path.join(THIS_DIR, "validate_scaffold.py"), "r", encoding="utf-8") as f:
            cls.source = f.read()

    def test_no_networking_imports(self):
        forbidden = ("import socket", "import requests", "import urllib", "import http.client", "import websocket")
        for token in forbidden:
            self.assertNotIn(token, self.source, f"forbidden import present: {token}")

    def test_no_subprocess_or_process_start(self):
        forbidden = ("import subprocess", "os.system(", "os.popen(", "os.spawn")
        for token in forbidden:
            self.assertNotIn(token, self.source, f"forbidden process-start capability present: {token}")

    def test_no_database_or_daemon_route_literals(self):
        forbidden = ("psycopg2", "sqlalchemy", "sqlx", "/api/v1/ops/action", "alpaca", "Start-MiniQuantDesk")
        lowered = self.source.lower()
        for token in forbidden:
            self.assertNotIn(token.lower(), lowered, f"forbidden runtime/broker/daemon reference present: {token}")

    def test_module_docstring_states_the_never_list(self):
        self.assertIn("MUST NEVER", self.source)
        self.assertIn("acquire a lease", self.source)
        self.assertIn("generate a fencing token", self.source)


if __name__ == "__main__":
    unittest.main()
