from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, Tuple


def load_walk_forward_eval(run_dir: Path) -> Dict[str, Any]:
    p = Path(run_dir) / "eval" / "walk_forward_eval.json"
    if not p.exists():
        raise FileNotFoundError(f"Missing {p}. Run mqk-ml-eval-wf first.")
    return json.loads(p.read_text(encoding="utf-8"))


def check_eval_gates(run_dir: Path, *, min_folds_used: int = 2, min_auc_mean: float = 0.52) -> Tuple[bool, str]:
    ev = load_walk_forward_eval(run_dir)
    s = ev.get("summary", {})
    folds_used = int(s.get("folds_used", 0))
    auc_mean = float(s.get("auc_mean", float("nan")))

    if folds_used < min_folds_used:
        return False, f"gate_fail: folds_used {folds_used} < {min_folds_used}"
    if auc_mean != auc_mean:
        return False, "gate_fail: auc_mean is NaN"
    if auc_mean < min_auc_mean:
        return False, f"gate_fail: auc_mean {auc_mean:.4f} < {min_auc_mean:.4f}"
    return True, "ok"
