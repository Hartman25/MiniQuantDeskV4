"""Read-only operator status of the Factory (queued, running, succeeded, failed, blocked, rejected work).

This is the integration seam for any operator surface: a deterministic JSON document with an explicit `truth_state`
(`active` | `no_db`) that is built from the control-plane file opened READ-ONLY (it never creates or migrates anything).
No daemon, GUI or Paper/Live component is touched here; a GUI/API consumer can poll the exported file.
"""

from __future__ import annotations

import json
import os
import sqlite3
from pathlib import Path
from typing import Any

from mqk_research.strategy_factory.contracts import FACTORY_AUTHORITY, promotion_view

SCHEMA = "factory_status_v1"


def build_status(db_path: Path) -> dict[str, Any]:
    db_path = Path(db_path)
    if not db_path.is_file():
        return {"schema": SCHEMA, "truth_state": "no_db", "reason": f"{db_path.name} does not exist; nothing is claimed"}
    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    try:
        campaigns = []
        for c in con.execute("select * from campaigns order by created_seq"):
            jobs = [dict(j) for j in con.execute("select * from jobs where campaign_id=? order by stage_order", (c["campaign_id"],))]
            spec = json.loads(c["spec_json"])
            campaigns.append({
                "campaign_id": c["campaign_id"], "state": c["state"], "state_reason": c["state_reason"], "evidence_grade": c["evidence_grade"],
                **promotion_view(c["evidence_grade"]), "declaration_sha256": c["declaration_sha256"],
                "trials": con.execute("select count(*) from campaign_trials where campaign_id=?", (c["campaign_id"],)).fetchone()[0],
                "population_symbols": sorted(spec["population"]["symbols"]),
                "stages": [{"stage": j["stage"], "status": j["status"], "attempts": j["attempt_count"], "reason": j["last_reason"]} for j in jobs]})
        counts: dict[str, int] = {}
        for r in con.execute("select status, count(*) n from jobs group by status"):
            counts[r["status"]] = r["n"]
        ideas: dict[str, int] = {}
        latest: dict[str, str] = {}
        for r in con.execute("select intake_id, record_json from idea_versions order by seq"):
            latest[r["intake_id"]] = r["record_json"]
        for body in latest.values():
            d = json.loads(body).get("disposition", "UNDISPOSITIONED")
            ideas[d] = ideas.get(d, 0) + 1
        ai: dict[str, int] = {}
        for r in con.execute("select status, count(*) n from ai_normalizations group by status"):
            ai[r["status"]] = r["n"]
        last_event = con.execute("select coalesce(max(seq),0) from events").fetchone()[0]
    except sqlite3.Error as exc:
        return {"schema": SCHEMA, "truth_state": "backend_unavailable", "reason": f"control-plane file unreadable: {exc}"}
    finally:
        con.close()
    return {"schema": SCHEMA, "truth_state": "active", "event_seq": last_event, "campaigns": campaigns, "job_counts": dict(sorted(counts.items())),
            "ideas_by_disposition": dict(sorted(ideas.items())), "ai_normalizations_by_status": dict(sorted(ai.items())),
            "authority": dict(FACTORY_AUTHORITY)}


def write_status(db_path: Path, out: Path) -> Path:
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(out.suffix + ".tmp")
    tmp.write_text(json.dumps(build_status(db_path), indent=1, sort_keys=True), encoding="utf-8")
    os.replace(tmp, out)
    return out
