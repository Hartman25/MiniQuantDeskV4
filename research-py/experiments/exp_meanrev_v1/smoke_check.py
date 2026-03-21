from __future__ import annotations

import json
import sys
from pathlib import Path

THIS_DIR = Path(__file__).resolve().parent
if str(THIS_DIR) not in sys.path:
    sys.path.insert(0, str(THIS_DIR))

import run as exp_run  # noqa: E402


def main() -> int:
    config_path = THIS_DIR / "config.yaml"
    config = exp_run._load_config(config_path)
    exp_run._validate_exp_only(config)
    spec = exp_run._to_spec(config)
    bars_path = exp_run._sample_bars_path(spec, config_path)

    run_id_a = exp_run._build_run_id(spec, config_path=config_path, bars_path=bars_path)
    run_id_b = exp_run._build_run_id(spec, config_path=config_path, bars_path=bars_path)
    if run_id_a != run_id_b:
        raise RuntimeError("Run ID must be deterministic for identical config and bars input.")

    output_root = exp_run._output_root(spec).resolve()
    expected_root = (THIS_DIR.parents[1] / "runs" / "EXP" / "exp_meanrev_v1").resolve()
    if output_root != expected_root:
        raise RuntimeError(
            f"EXP output root escaped containment: expected={expected_root} got={output_root}"
        )

    result = exp_run.run_pipeline(config_path)
    if result.run_dir.parent.resolve() != expected_root:
        raise RuntimeError(
            f"EXP run dir escaped containment: expected_parent={expected_root} got={result.run_dir.parent.resolve()}"
        )

    summary = {
        "ok": True,
        "engine_id": spec.engine_id,
        "canonical": False,
        "readiness_bearing": False,
        "operator_visible": False,
        "capital_authoritative": False,
        "invocation": "explicit_local_only",
        "run_id": run_id_a,
        "output_root": str(output_root),
        "bars_sha256": exp_run._sha256_file(bars_path),
        "config_sha256": exp_run._sha256_file(config_path),
    }

    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
