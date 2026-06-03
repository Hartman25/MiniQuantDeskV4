"""
EXP-PENNY-01A-JOURNAL-REVIEW — read-only candidate journal review tool.

Reads JSONL files written by CandidateJournalWriter. Produces pass/reject
counts, rejection reason tallies, top candidate ranking, safety checks,
and an optional markdown + JSON summary.

Safety invariants:
- Read-only. No file writes except summary outputs under exports/experimental/reviews/.
- No network imports (requests, urllib, http.client, aiohttp).
- No broker adapter imports.
- No OMS / outbox / inbox imports.
- No DB imports.
- Fails if paper_order_id or live_order_id are non-null (OPEN-SAFETY-FAIL).

Usage:
    $env:PYTHONPATH="research-py"
    python -m experiments.exp_penny.review_journal --latest
    python -m experiments.exp_penny.review_journal --journal exports\\experimental\\candidates\\<file>.jsonl
    python -m experiments.exp_penny.review_journal --journal-dir exports\\experimental\\candidates --write-summary
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

DEFAULT_JOURNAL_DIR = "exports/experimental/candidates"
DEFAULT_REVIEW_DIR = "exports/experimental/reviews"
REQUIRED_FIELDS = [
    "schema_version",
    "engine_id",
    "lane_id",
    "strategy_id",
    "scanned_at_utc",
    "symbol",
    "asset_class",
    "candidate_type",
    "signal_direction",
    "would_trade",
    "paper_order_id",
    "live_order_id",
]
ALLOWED_SIGNAL_DIRECTIONS = {"long", "neutral"}
EXPECTED_ASSET_CLASS = "us_equity"

# Classification constants
CLASS_CLOSED_SCANNER_ONLY_PASS = "CLOSED-SCANNER-ONLY-PASS"
CLASS_CLOSED_NO_CANDIDATES = "CLOSED-NO-CANDIDATES"
CLASS_OPEN_SAFETY_FAIL = "OPEN-SAFETY-FAIL"
CLASS_OPEN_EMPTY_JOURNAL = "OPEN-EMPTY-JOURNAL"
CLASS_PARTIAL = "PARTIAL"


# ---------------------------------------------------------------------------
# Data classes (stdlib only — no dataclasses import needed for simple structs)
# ---------------------------------------------------------------------------


class ReviewResult:
    """Holds the full review output for a single journal file."""

    def __init__(self) -> None:
        self.journal_path: str = ""
        self.total_rows: int = 0
        self.valid_rows: int = 0
        self.malformed_rows: int = 0
        self.would_trade_true: int = 0
        self.would_trade_false: int = 0
        self.rejection_reason_counts: Counter = Counter()
        self.signal_direction_counts: Counter = Counter()
        self.asset_class_counts: Counter = Counter()
        self.strategy_id_counts: Counter = Counter()
        self.safety_failures: list[str] = []
        self.warnings: list[str] = []
        self.top_candidates: list[dict[str, Any]] = []
        self.classification: str = CLASS_OPEN_EMPTY_JOURNAL
        self.summary_md_path: Optional[str] = None
        self.summary_json_path: Optional[str] = None


# ---------------------------------------------------------------------------
# Journal loading
# ---------------------------------------------------------------------------


def load_journal(journal_path: Path) -> tuple[list[dict[str, Any]], list[str]]:
    """
    Load a JSONL file. Returns (valid_records, malformed_line_errors).
    Raises FileNotFoundError if the path does not exist.
    """
    records: list[dict[str, Any]] = []
    errors: list[str] = []

    with open(journal_path, "r", encoding="utf-8") as fh:
        for lineno, raw in enumerate(fh, start=1):
            line = raw.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
                if not isinstance(record, dict):
                    errors.append(f"line {lineno}: not a JSON object")
                    continue
                records.append(record)
            except json.JSONDecodeError as exc:
                errors.append(f"line {lineno}: {exc}")

    return records, errors


def find_latest_journal(journal_dir: Path) -> Optional[Path]:
    """Return the most recently modified .jsonl file in journal_dir, or None."""
    candidates = sorted(journal_dir.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)
    return candidates[0] if candidates else None


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------


def _check_required_fields(record: dict[str, Any], lineno: int) -> Optional[str]:
    """Return an error string if any required field is missing, else None."""
    missing = [f for f in REQUIRED_FIELDS if f not in record]
    if missing:
        return f"record #{lineno} missing required fields: {missing}"
    return None


def validate_record(record: dict[str, Any], lineno: int) -> list[str]:
    """Return a list of field-level validation errors for a record."""
    errors: list[str] = []
    err = _check_required_fields(record, lineno)
    if err:
        errors.append(err)
    return errors


# ---------------------------------------------------------------------------
# Review logic
# ---------------------------------------------------------------------------


def review_journal(
    journal_path: Path,
    top_n: int = 10,
    min_confidence: Optional[float] = None,
) -> ReviewResult:
    """
    Core review function. Pure — does not write any files.

    Returns a ReviewResult with all counts, safety checks, and top candidates.
    """
    result = ReviewResult()
    result.journal_path = str(journal_path)

    try:
        records, malformed = load_journal(journal_path)
    except FileNotFoundError:
        result.safety_failures.append(f"Journal file not found: {journal_path}")
        result.classification = CLASS_OPEN_SAFETY_FAIL
        return result

    result.total_rows = len(records) + len(malformed)
    result.malformed_rows = len(malformed)
    for err in malformed:
        result.safety_failures.append(f"Malformed line: {err}")

    schema_errors: list[str] = []
    valid_records: list[dict[str, Any]] = []

    for idx, record in enumerate(records, start=1):
        field_errors = validate_record(record, idx)
        if field_errors:
            schema_errors.extend(field_errors)
            continue
        valid_records.append(record)

    result.valid_rows = len(valid_records)

    for err in schema_errors:
        result.safety_failures.append(f"Schema error: {err}")

    # Safety checks — order IDs must always be null
    for idx, record in enumerate(valid_records, start=1):
        if record.get("paper_order_id") is not None:
            result.safety_failures.append(
                f"record #{idx} ({record.get('symbol')}): paper_order_id is non-null "
                f"({record['paper_order_id']!r}) — scanner-only invariant violated"
            )
        if record.get("live_order_id") is not None:
            result.safety_failures.append(
                f"record #{idx} ({record.get('symbol')}): live_order_id is non-null "
                f"({record['live_order_id']!r}) — scanner-only invariant violated"
            )

    # Counts
    for record in valid_records:
        wt = record.get("would_trade", False)
        if wt:
            result.would_trade_true += 1
        else:
            result.would_trade_false += 1

        rr = record.get("rejection_reason")
        if rr:
            result.rejection_reason_counts[rr] += 1

        sd = record.get("signal_direction", "unknown")
        result.signal_direction_counts[sd] += 1

        ac = record.get("asset_class", "unknown")
        result.asset_class_counts[ac] += 1

        sid = record.get("strategy_id", "unknown")
        result.strategy_id_counts[sid] += 1

    # Warnings
    for ac in result.asset_class_counts:
        if ac != EXPECTED_ASSET_CLASS:
            result.warnings.append(
                f"Unexpected asset_class '{ac}' found — scanner is us_equity only"
            )
    for sd in result.signal_direction_counts:
        if sd not in ALLOWED_SIGNAL_DIRECTIONS:
            result.warnings.append(
                f"Unexpected signal_direction '{sd}' — expected long or neutral"
            )

    # Top candidates ranking:
    # would_trade=True first, then confidence_score descending (None last)
    passing = [r for r in valid_records if r.get("would_trade", False)]
    if min_confidence is not None:
        passing = [
            r for r in passing
            if r.get("confidence_score") is not None and r["confidence_score"] >= min_confidence
        ]
    passing_sorted = sorted(
        passing,
        key=lambda r: (r.get("confidence_score") is not None, r.get("confidence_score") or 0.0),
        reverse=True,
    )
    result.top_candidates = passing_sorted[:top_n]

    # Classification
    if result.safety_failures:
        result.classification = CLASS_OPEN_SAFETY_FAIL
    elif result.total_rows == 0:
        result.classification = CLASS_OPEN_EMPTY_JOURNAL
    elif result.valid_rows == 0:
        result.classification = CLASS_OPEN_SAFETY_FAIL
    elif result.malformed_rows > 0 or schema_errors:
        result.classification = CLASS_PARTIAL
    elif result.warnings:
        result.classification = CLASS_PARTIAL
    elif result.would_trade_true == 0:
        result.classification = CLASS_CLOSED_NO_CANDIDATES
    else:
        result.classification = CLASS_CLOSED_SCANNER_ONLY_PASS

    return result


# ---------------------------------------------------------------------------
# Summary formatting
# ---------------------------------------------------------------------------


def _format_markdown(result: ReviewResult) -> str:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lines = [
        "# Penny Scanner Journal Review",
        "",
        f"**Journal:** `{result.journal_path}`",
        f"**Reviewed at:** {now}",
        f"**Classification:** `{result.classification}`",
        "",
        "## Counts",
        "",
        f"| Metric | Value |",
        f"|--------|-------|",
        f"| Total rows | {result.total_rows} |",
        f"| Valid rows | {result.valid_rows} |",
        f"| Malformed rows | {result.malformed_rows} |",
        f"| would_trade = true | {result.would_trade_true} |",
        f"| would_trade = false | {result.would_trade_false} |",
        "",
    ]

    if result.rejection_reason_counts:
        lines += [
            "## Rejection Reasons",
            "",
            "| Reason | Count |",
            "|--------|-------|",
        ]
        for reason, count in result.rejection_reason_counts.most_common():
            lines.append(f"| {reason} | {count} |")
        lines.append("")

    if result.safety_failures:
        lines += ["## Safety Failures", ""]
        for sf in result.safety_failures:
            lines.append(f"- **FAIL**: {sf}")
        lines.append("")

    if result.warnings:
        lines += ["## Warnings", ""]
        for w in result.warnings:
            lines.append(f"- WARNING: {w}")
        lines.append("")

    if result.top_candidates:
        lines += [
            "## Top Candidates",
            "",
            "| Symbol | confidence_score | rejection_reason | signal_direction |",
            "|--------|-----------------|-----------------|-----------------|",
        ]
        for c in result.top_candidates:
            lines.append(
                f"| {c.get('symbol')} | {c.get('confidence_score')} "
                f"| {c.get('rejection_reason')} | {c.get('signal_direction')} |"
            )
        lines.append("")

    lines += [
        "---",
        "",
        "> This review does not prove profitability or readiness for execution.",
        "> A valid CLOSED-SCANNER-ONLY-PASS classification is required before backtest",
        "> or paper execution lanes (Stage 2+) may begin.",
        "",
    ]

    return "\n".join(lines)


def _format_json(result: ReviewResult) -> dict[str, Any]:
    return {
        "schema_version": "review-v1",
        "journal_path": result.journal_path,
        "reviewed_at_utc": datetime.now(timezone.utc).isoformat(),
        "classification": result.classification,
        "counts": {
            "total_rows": result.total_rows,
            "valid_rows": result.valid_rows,
            "malformed_rows": result.malformed_rows,
            "would_trade_true": result.would_trade_true,
            "would_trade_false": result.would_trade_false,
        },
        "rejection_reason_counts": dict(result.rejection_reason_counts.most_common()),
        "signal_direction_counts": dict(result.signal_direction_counts),
        "asset_class_counts": dict(result.asset_class_counts),
        "strategy_id_counts": dict(result.strategy_id_counts),
        "safety_failures": result.safety_failures,
        "warnings": result.warnings,
        "top_candidates": [
            {
                "symbol": c.get("symbol"),
                "confidence_score": c.get("confidence_score"),
                "signal_direction": c.get("signal_direction"),
                "rejection_reason": c.get("rejection_reason"),
                "extra": c.get("extra"),
            }
            for c in result.top_candidates
        ],
    }


def write_summaries(result: ReviewResult, review_dir: Path) -> None:
    """Write markdown and JSON summaries. Sets result.summary_*_path."""
    review_dir.mkdir(parents=True, exist_ok=True)
    stem = Path(result.journal_path).stem
    md_path = review_dir / f"{stem}_review.md"
    json_path = review_dir / f"{stem}_review.json"

    md_path.write_text(_format_markdown(result), encoding="utf-8")
    json_path.write_text(
        json.dumps(_format_json(result), indent=2, default=str),
        encoding="utf-8",
    )
    result.summary_md_path = str(md_path)
    result.summary_json_path = str(json_path)


# ---------------------------------------------------------------------------
# Console output
# ---------------------------------------------------------------------------


def print_summary(result: ReviewResult) -> None:
    print()
    print("=" * 60)
    print("EXP-PENNY-01A Journal Review")
    print("=" * 60)
    print(f"Journal    : {result.journal_path}")
    print(f"Classification: {result.classification}")
    print()
    print(f"Total rows     : {result.total_rows}")
    print(f"Valid rows     : {result.valid_rows}")
    print(f"Malformed rows : {result.malformed_rows}")
    print(f"would_trade=T  : {result.would_trade_true}")
    print(f"would_trade=F  : {result.would_trade_false}")

    if result.rejection_reason_counts:
        print()
        print("Rejection reasons:")
        for reason, count in result.rejection_reason_counts.most_common(10):
            print(f"  {count:4d}  {reason}")

    if result.safety_failures:
        print()
        print("SAFETY FAILURES:")
        for sf in result.safety_failures:
            print(f"  FAIL: {sf}")

    if result.warnings:
        print()
        print("Warnings:")
        for w in result.warnings:
            print(f"  WARN: {w}")

    if result.top_candidates:
        print()
        print(f"Top {len(result.top_candidates)} candidates (would_trade=True):")
        for c in result.top_candidates:
            conf = c.get("confidence_score")
            conf_str = f"{conf:.4f}" if conf is not None else "n/a"
            print(f"  {c.get('symbol'):10s}  conf={conf_str}  dir={c.get('signal_direction')}")

    if result.summary_md_path:
        print()
        print(f"Summary MD  : {result.summary_md_path}")
        print(f"Summary JSON: {result.summary_json_path}")

    print()
    print("=" * 60)
    print(f"Classification: {result.classification}")
    print("=" * 60)
    print()


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="EXP-PENNY-01A-JOURNAL-REVIEW — read-only candidate journal reviewer",
        add_help=True,
    )
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--journal",
        metavar="PATH",
        help="Specific JSONL journal file to review.",
    )
    group.add_argument(
        "--latest",
        action="store_true",
        help="Select the latest JSONL file in --journal-dir.",
    )
    parser.add_argument(
        "--journal-dir",
        default=DEFAULT_JOURNAL_DIR,
        metavar="DIR",
        help=f"Directory containing candidate JSONL files. Default: {DEFAULT_JOURNAL_DIR}",
    )
    parser.add_argument(
        "--write-summary",
        action="store_true",
        help="Write markdown and JSON summaries to exports/experimental/reviews/.",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        metavar="N",
        help="Number of top candidates to display. Default: 10",
    )
    parser.add_argument(
        "--min-confidence",
        type=float,
        default=None,
        metavar="FLOAT",
        help="Minimum confidence_score for top candidates filter.",
    )
    parser.add_argument(
        "--no-fail-on-order-ids",
        action="store_true",
        help="Disable the safety failure on non-null order IDs (not recommended).",
    )
    args = parser.parse_args(argv)

    # Resolve journal path
    journal_path: Optional[Path] = None

    if args.journal:
        journal_path = Path(args.journal)
    elif args.latest:
        journal_dir = Path(args.journal_dir)
        if not journal_dir.exists():
            print(f"ERROR: journal directory not found: {journal_dir}", file=sys.stderr)
            return 1
        journal_path = find_latest_journal(journal_dir)
        if journal_path is None:
            print(f"ERROR: no JSONL files found in {journal_dir}", file=sys.stderr)
            return 1
    else:
        # Default: show all journals in dir (or error)
        print("ERROR: specify --journal <path> or --latest", file=sys.stderr)
        parser.print_help()
        return 1

    result = review_journal(
        journal_path=journal_path,
        top_n=args.top,
        min_confidence=args.min_confidence,
    )

    if args.write_summary:
        review_dir = Path(DEFAULT_REVIEW_DIR)
        write_summaries(result, review_dir)

    print_summary(result)

    # Exit non-zero on safety failures
    if result.safety_failures and not args.no_fail_on_order_ids:
        return 2

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
