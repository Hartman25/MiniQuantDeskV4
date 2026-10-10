"""Stage executor: runs ONE campaign stage through the accepted stage-authorized batch runner, as a subprocess.

The Factory owns no economics. A job is `python run_batch.py <stage> --resume` with the campaign declaration named by
MQK_M1_BATCH_DECLARATION, so every accepted guard (stage authorization HMAC bound to the declaration identity and the
native binary hash, holdout incident ledger, registration gate, causal bridge, judge, scanner/review) runs unchanged.
Before spawning, a read-only preflight verifies the same prerequisites and reports precisely what is missing:
a missing authorization is BLOCKED (re-evaluable), a declaration that no longer matches its frozen hash is FAILED.
The Factory never mints an authorization, never sets the execution gate, and never passes credentials in arguments.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Mapping

from mqk_research.strategy_factory.campaign import EXPERIMENTS_REL, declaration_identity

RUNNER_STAGE = {"check": "check", "data": "reuse_data", "register": "register", "gate": "gate", "trials": "trials",
                "judge": "judge", "backtest": "backtest", "finalize": "finalize", "review": "review", "summary": "summary"}
GUARD_STAGES = {"holdout_pre": "pre", "holdout_post": "post"}
INTERNAL_STAGES = ("report",)
NATIVE_STAGES = ("register", "gate", "trials", "backtest", "finalize", "review")      # execute the pinned native binary
AUTH_FILE_ENV = "MQK_M1_STAGE_AUTHORIZATION"
KEY_ENV = "MQK_M1_STAGE_AUTH_KEY"
CLI_ENV = "MQK_M1_CLI"


@dataclass(frozen=True)
class Outcome:
    status: str                # succeeded | failed | blocked
    exit_code: int | None
    reason: str | None
    output: str = ""


def _load_sa(repo_root: Path):
    """The accepted authorization module (read-only use: verify, never mint)."""
    exp = str(Path(repo_root) / EXPERIMENTS_REL)
    if exp not in sys.path:
        sys.path.insert(0, exp)
    import stage_authorization  # noqa: PLC0415 - lives next to the runner by design
    return stage_authorization


class StageExecutor:
    def __init__(self, repo_root: Path, *, cli_path: Path | None = None, python: str | None = None, env: Mapping[str, str] | None = None,
                 stage_timeout: float = 4 * 3600.0, clock: Callable[[], datetime] = lambda: datetime.now(timezone.utc)) -> None:
        self.repo_root = Path(repo_root).resolve()
        self.exp_dir = self.repo_root / EXPERIMENTS_REL
        self.cli_path = Path(cli_path) if cli_path else None
        self.python = python or sys.executable
        self._env = dict(os.environ if env is None else env)
        self.stage_timeout = stage_timeout
        self.clock = clock
        self.store = None            # attached by the scheduler so the report can include restart/retry history

    def auth_path(self, campaign: Mapping[str, Any]) -> str | None:
        """Per-campaign signed authorization (operator-placed `<run_dir>/stage_authorization.json`) wins over the shared environment
        path, so independent campaigns with independent authorizations can run in one pass."""
        run_dir = campaign.get("run_dir")
        local = Path(run_dir) / "stage_authorization.json" if run_dir else None
        return str(local) if local is not None and local.is_file() else self._env.get(AUTH_FILE_ENV)

    # ------------------------------------------------------------------ preflight
    def load_declaration(self, campaign: Mapping[str, Any]) -> dict[str, Any]:
        path = Path(campaign["declaration_path"])
        decl = json.loads(path.read_text(encoding="utf-8"))
        if declaration_identity(decl) != campaign["declaration_sha256"]:
            raise ValueError("DECLARATION_IDENTITY_MISMATCH")
        return decl

    def preflight(self, campaign: Mapping[str, Any], stage: str) -> Outcome | None:
        """None = clear to run; otherwise a blocked/failed Outcome that explains exactly why not."""
        if stage in INTERNAL_STAGES:
            return None
        try:
            decl = self.load_declaration(campaign)
        except (OSError, ValueError) as exc:
            return Outcome("failed", None, f"declaration unreadable or altered since predeclaration: {exc}")
        gate = decl.get("execution_gate") or {}
        if gate.get("executable") is not True:
            return Outcome("blocked", None, f"BLOCKED_GATE: execution gate not released ({gate.get('status')}; {gate.get('blocker')})")
        sa = _load_sa(self.repo_root)
        if stage not in RUNNER_STAGE and stage not in GUARD_STAGES:
            return Outcome("failed", None, f"unknown stage {stage!r}")
        auth_class = sa.STAGE_CLASS[RUNNER_STAGE[stage]] if stage in RUNNER_STAGE else sa.READ_ONLY
        auth = sa.load_auth_file(self.auth_path(campaign))
        key = self._env.get(KEY_ENV)
        if auth_class != sa.READ_ONLY:
            try:
                sa.verify(decl, auth_class, auth, key=key, now=self.clock())
            except sa.AuthorizationError as exc:
                return Outcome("blocked", None, f"BLOCKED_AUTHORIZATION: {exc}")
        if stage in NATIVE_STAGES:
            if self.cli_path is None:
                return Outcome("blocked", None, "BLOCKED_DEPENDENCY: no native mqk-cli binary configured")
            try:
                sa.verified_cli(auth, self.cli_path)
            except sa.AuthorizationError as exc:
                return Outcome("blocked", None, f"BLOCKED_AUTHORIZATION: {exc}")
        return None

    # ------------------------------------------------------------------ execution
    def _command(self, stage: str, attempt_no: int) -> list[str]:
        if stage in GUARD_STAGES:
            return [self.python, str(self.exp_dir / "holdout_guard.py"), GUARD_STAGES[stage]]
        return [self.python, str(self.exp_dir / "run_batch.py"), RUNNER_STAGE[stage], "--resume"]

    def execute(self, campaign: Mapping[str, Any], stage: str, attempt_no: int, *, heartbeat: Callable[[], None] | None = None,
                heartbeat_every: float = 60.0) -> Outcome:
        blocked = self.preflight(campaign, stage)
        if blocked is not None:
            return blocked
        if stage in INTERNAL_STAGES:
            from mqk_research.strategy_factory.reporting import write_campaign_report
            try:
                path = write_campaign_report(campaign, self.repo_root, self.store)
            except Exception as exc:                                           # noqa: BLE001 - reported, never swallowed
                return Outcome("failed", None, f"report assembly failed: {type(exc).__name__}: {exc}")
            return Outcome("succeeded", 0, None, f"report written: {path}")
        env = dict(self._env)
        env["MQK_M1_BATCH_DECLARATION"] = campaign["declaration_path"]
        if self.auth_path(campaign):
            env[AUTH_FILE_ENV] = self.auth_path(campaign)
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        if self.cli_path is not None:
            env[CLI_ENV] = str(self.cli_path)
        stop = threading.Event()
        beat = None
        if heartbeat is not None:
            def loop() -> None:
                while not stop.wait(heartbeat_every):
                    try:
                        heartbeat()
                    except Exception:                                          # noqa: BLE001 - a lost claim ends the job via finish()
                        return
            beat = threading.Thread(target=loop, daemon=True)
            beat.start()
        try:
            proc = subprocess.run(self._command(stage, attempt_no), cwd=self.exp_dir, env=env, capture_output=True, text=True,
                                  timeout=self.stage_timeout)
        except subprocess.TimeoutExpired as exc:
            return Outcome("failed", None, f"stage exceeded {self.stage_timeout:.0f}s", (exc.stdout or "")[-2000:] if isinstance(exc.stdout, str) else "")
        finally:
            stop.set()
            if beat is not None:
                beat.join(timeout=2)
        out = (proc.stdout or "") + (("\n[stderr]\n" + proc.stderr) if proc.stderr else "")
        if proc.returncode == 0:
            return Outcome("succeeded", 0, None, out)
        tail = (proc.stderr or proc.stdout or "").strip().splitlines()[-1:] or ["non-zero exit"]
        text = tail[0][:400]
        if "authorization" in text.lower() or "credentials unavailable" in text.lower():
            return Outcome("blocked", proc.returncode, f"BLOCKED_AUTHORIZATION: {text}", out)
        return Outcome("failed", proc.returncode, text, out)

    # ------------------------------------------------------------------ readiness
    def readiness(self, campaign: Mapping[str, Any]) -> dict[str, Any]:
        """Per-prerequisite truth for a campaign, without running anything. Each entry is OK | BLOCKED | FAILED with a reason."""
        rows: dict[str, dict[str, str]] = {}

        def put(name: str, ok: bool, detail: str, kind: str = "BLOCKED") -> None:
            rows[name] = {"status": "OK" if ok else kind, "detail": detail}

        try:
            decl = self.load_declaration(campaign)
        except (OSError, ValueError) as exc:
            put("declaration", False, str(exc), "FAILED")
            return {"campaign_id": campaign["campaign_id"], "ready": False, "prerequisites": rows}
        put("declaration", True, f"identity {campaign['declaration_sha256'][:16]} matches the frozen predeclaration")
        gate = decl.get("execution_gate") or {}
        put("execution_gate", gate.get("executable") is True, f"{gate.get('status')} / {gate.get('blocker')}")
        sa = _load_sa(self.repo_root)
        auth = sa.load_auth_file(self.auth_path(campaign))
        key = self._env.get(KEY_ENV)
        classes = sorted({sa.STAGE_CLASS[s] for s in RUNNER_STAGE.values() if sa.STAGE_CLASS[s] != sa.READ_ONLY})
        for c in classes:
            try:
                sa.verify(decl, c, auth, key=key, now=self.clock())
                put(f"authorization:{c}", True, "valid signed authorization")
            except sa.AuthorizationError as exc:
                put(f"authorization:{c}", False, str(exc))
        cli_ok = self.cli_path is not None and self.cli_path.is_file()
        put("native_binary", cli_ok, str(self.cli_path) if cli_ok else "no native mqk-cli binary at the configured path")
        src = Path(decl["data"]["reuse_verified_data_from"]["run_dir"])
        manifest = src / "research_bars_provenance.json"
        if manifest.is_file():
            m = json.loads(manifest.read_text(encoding="utf-8"))
            pin = decl["data"]["reuse_verified_data_from"]
            put("data_pins", m.get("artifact_sha256") == pin["expected_artifact_sha256"] and m.get("row_count") == pin["expected_row_count"]
                and m.get("canonical_semantic_bars_hash") == pin["expected_canonical_semantic_bars_hash"],
                "manifest pins compared with the declaration", "FAILED")
            authority = (m.get("source_attestation") or {}).get("source_authority")
            grade = decl["evidence_grade"]["grade"]
            put("data_authority", (authority == "official_provider") == (grade != "SYNTHETIC_DIAGNOSTIC"),
                f"source_authority={authority!r} vs evidence grade {grade}", "FAILED")
        else:
            put("data_pins", False, f"no verified bars manifest at {manifest}")
        try:
            from mqk_research.strategy_factory import holdout_view
            put("holdout_incidents", True, json.dumps(holdout_view.summary(self.repo_root, decl), sort_keys=True))
        except Exception as exc:                                                # noqa: BLE001
            put("holdout_incidents", False, f"incident ledger unreadable: {exc}", "FAILED")
        ready = all(r["status"] == "OK" for r in rows.values())
        return {"campaign_id": campaign["campaign_id"], "ready": ready, "prerequisites": rows}
