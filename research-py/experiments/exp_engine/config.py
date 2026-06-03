"""
EXP-ENGINE-CORE-01 — experimental engine config gate.

All flags default to the most-restrictive value. The engine is disabled unless
MQK_EXPERIMENTAL_ENGINE_ENABLED=true is explicitly set in the environment.

This module has NO imports from broker adapters, OMS, or the main orchestrator.
"""
from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class ExperimentalEngineConfig:
    enabled: bool
    mode: str               # "scanner_only" — only valid value in Stage 1
    live_allowed: bool      # always False; enforced structurally
    journal_dir: str        # output path for candidate JSONL files

    def is_execution_allowed(self) -> bool:
        """Stage 1 is scanner-only. Execution is structurally blocked."""
        return False

    def validate(self) -> None:
        if self.live_allowed:
            raise ValueError(
                "MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED must not be true — "
                "live execution is not supported in Stage 1."
            )
        if self.mode != "scanner_only":
            raise ValueError(
                f"MQK_EXPERIMENTAL_ENGINE_MODE must be 'scanner_only'; got {self.mode!r}. "
                "Execution modes are not available in Stage 1."
            )


def load_config() -> ExperimentalEngineConfig:
    """
    Load experimental engine config from environment.

    Returns a disabled config when MQK_EXPERIMENTAL_ENGINE_ENABLED is absent or false.
    Never raises on a disabled config — caller checks cfg.enabled before running.
    """
    enabled_raw = os.environ.get("MQK_EXPERIMENTAL_ENGINE_ENABLED", "false").strip().lower()
    enabled = enabled_raw == "true"

    mode = os.environ.get("MQK_EXPERIMENTAL_ENGINE_MODE", "scanner_only").strip()
    live_allowed_raw = os.environ.get("MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED", "false").strip().lower()
    live_allowed = live_allowed_raw == "true"

    journal_dir = os.environ.get(
        "MQK_EXPERIMENTAL_JOURNAL_DIR",
        "exports/experimental/candidates",
    ).strip()

    cfg = ExperimentalEngineConfig(
        enabled=enabled,
        mode=mode,
        live_allowed=live_allowed,
        journal_dir=journal_dir,
    )

    if enabled:
        cfg.validate()

    return cfg
