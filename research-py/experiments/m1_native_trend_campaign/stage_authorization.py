"""Per-stage operator authorization for new graded campaigns (`m1_stage_authorization_v1`).

`execution_gate.executable = true` is a mutable flag; on its own it must not authorize a provider call,
a registration, an attempt, a judge or a promotion. A stage of a non-historical declaration additionally
needs an authorization that

* is bound to the exact declaration identity (everything except `execution_gate`, so re-issuing the gate
  does not change it, while any economic/protocol/data change does),
* names the stage class it authorizes (a fetch authorization never authorizes an attempt),
* names an operator and an approval reference, carries a bounded validity window, and acknowledges every
  pending Final-Holdout access incident that affects the declaration,
* is authenticated by an HMAC-SHA256 under an operator-held secret (`MQK_M1_STAGE_AUTH_KEY`).

Enforceable boundary, stated plainly: the HMAC proves the authorization was minted by a holder of the
secret and not altered; it is a shared-secret check, not non-repudiation, and it cannot stop someone who
holds the key. The secret lives outside the repository and outside the test environment, so a flipped
flag, a mutated test or a direct stage call cannot supply it. Historical frozen declarations are matched
by canonical content hash and keep their previous behaviour; anything not in that pinned set fails closed
into the authorized path.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path

import holdout_incident

HERE = Path(__file__).resolve().parent
SCHEMA = "m1_stage_authorization_v1"
KEY_ENV = "MQK_M1_STAGE_AUTH_KEY"
AUTH_FILE_ENV = "MQK_M1_STAGE_AUTHORIZATION"
MIN_KEY_CHARS = 32
MAX_VALIDITY = timedelta(days=7)

READ_ONLY = "read_only_verification"
DATA_MATERIALIZATION = "data_materialization"
NATIVE_IDENTITY_RESOLUTION = "native_identity_resolution"
PROVIDER_FETCH = "provider_fetch"
REGISTRATION = "registry_registration"
ATTEMPT = "attempt_execution"
JUDGE_FINALIZE = "judge_finalize"
PROMOTION = "promotion"
PAPER = "paper_deployment"
CLASSES = (READ_ONLY, DATA_MATERIALIZATION, NATIVE_IDENTITY_RESOLUTION, PROVIDER_FETCH, REGISTRATION, ATTEMPT, JUDGE_FINALIZE, PROMOTION, PAPER)
# Read-only verification never needs an authorization artifact; every other class does.
AUTHORIZABLE = tuple(c for c in CLASSES if c != READ_ONLY)
# Promotion and Paper depend on the holdout being independently clear; a pending or consumed window refuses them.
INCIDENT_BLOCKED = (PROMOTION, PAPER)

STAGE_CLASS = {"check": READ_ONLY, "gate": NATIVE_IDENTITY_RESOLUTION, "summary": READ_ONLY, "reuse_data": DATA_MATERIALIZATION,
               "fetch": PROVIDER_FETCH, "register": REGISTRATION, "trials": ATTEMPT, "backtest": ATTEMPT,
               "judge": JUDGE_FINALIZE, "finalize": JUDGE_FINALIZE, "review": JUDGE_FINALIZE}

# canonical sha256 of every frozen historical declaration that predates stage authorization.
HISTORICAL_DECLARATION_SHA256 = {
    "PREDECLARED_BATCH_01.json": "17dc8a8960087cbdef02b3518d6be519319c42902e561bd7690be3b0f77c00f8",
    "PREDECLARED_BATCH_01_CORRECTED.json": "fc8fcf55c51edaa3acafdaa109fdecdde22682abb7969da19f8416ceb8c5c67b",
    "PREDECLARED_BATCH_02.json": "c591bba4e75271f9ed8ff26b6659320ba1340fe4a4ea2c8c286b3b549d683e16",
    "PREDECLARED_BATCH_02_ERRATUM.json": "dfe09dbabe2456e17ddb17ea95c0ee7fa7bc35c1b712842b099802f9aded03a3",
    "PREDECLARED_BATCH_03.json": "b1f51fdfb5f8d569e3f4104ede7492e567c35aa115f657ee41c7fa62bdc73c76",
    "PREDECLARED_CAMPAIGN.json": "e92064cde876f6a0abb5aaab256a5c05e9b4cb1d62aad8a3a5a4c88eaaed0b2a",
    "PREDECLARED_CAMPAIGN_02.json": "071ca4443006553eabe493108cccf2f6c596e965630ac501cd6f82b3ebf7be79",
    "PREDECLARED_CAMPAIGN_03.json": "ae746e622605227e46defb6cb465bfe99e8eacef7c8ab2d497aa8847d050ec12",
    "PREDECLARED_CAMPAIGN_DUAL_SMA_01.json": "f0f6e82abf5a6606b43bc4441a7714c5e809059333ba2042b1dafdb2da34b5b5",
    "PREDECLARED_CAMPAIGN_PULLBACK_01.json": "3a54c17a75723647c003d951dff5b576e107b3f97db959fd3f3cb03a78e2ad8c",
}


class AuthorizationError(SystemExit):
    """Fail-closed refusal. A SystemExit so a stage aborts like every other runner refusal."""


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _campaign(decl: dict) -> str:
    return decl.get("campaign_id") or decl["batch_id"]


def declaration_identity(decl: dict) -> str:
    """sha256 over the whole declaration except `execution_gate` (the only block a re-issue may change)."""
    body = {k: v for k, v in decl.items() if k != "execution_gate"}
    return hashlib.sha256(canonical(body).encode("utf-8")).hexdigest()


def _content_sha256(decl: dict) -> str:
    return hashlib.sha256(canonical(decl).encode("utf-8")).hexdigest()


def is_frozen_historical(decl: dict) -> bool:
    """True only for a declaration whose exact content is one of the pinned pre-authorization campaigns."""
    return _content_sha256(decl) in set(HISTORICAL_DECLARATION_SHA256.values())


def _signature(auth: dict, key: str) -> str:
    body = {k: v for k, v in auth.items() if k != "signature"}
    return hmac.new(key.encode("utf-8"), canonical(body).encode("utf-8"), hashlib.sha256).hexdigest()


def mint(decl: dict, classes: list[str], *, operator: str, approval_ref: str, key: str, now: datetime,
         valid_for: timedelta = timedelta(hours=24), acknowledged_incidents: list[str] | None = None,
         acknowledged_data_boundaries: list[str] | None = None, cli_sha256: str | None = None) -> dict:
    """Operator tool (needs the secret). The controller never calls this outside tests. Incident
    acknowledgement is explicit: nothing is acknowledged unless the operator names it."""
    if len(key) < MIN_KEY_CHARS:
        raise AuthorizationError(f"fail-closed: {KEY_ENV} must be at least {MIN_KEY_CHARS} characters")
    if not set(classes) <= set(AUTHORIZABLE) or not classes:
        raise AuthorizationError(f"fail-closed: classes must be a non-empty subset of {AUTHORIZABLE}")
    if valid_for > MAX_VALIDITY or valid_for <= timedelta(0):
        raise AuthorizationError("fail-closed: validity must be positive and at most 7 days")
    auth = {"schema": SCHEMA, "campaign_id": _campaign(decl), "declaration_identity_sha256": declaration_identity(decl),
            "authorized_classes": sorted(classes), "operator": operator, "approval_ref": approval_ref,
            "issued_utc": now.astimezone(timezone.utc).isoformat(),
            "expires_utc": (now + valid_for).astimezone(timezone.utc).isoformat(),
            "acknowledged_incident_ids": sorted(acknowledged_incidents or []),
            "acknowledged_data_boundaries": sorted(acknowledged_data_boundaries or [])}
    if cli_sha256 is not None:  # binds the exact native binary the stage may execute (a mutable path proves nothing)
        auth["cli_sha256"] = cli_sha256
    auth["signature"] = _signature(auth, key)
    return auth


def verify(decl: dict, auth_class: str, auth: dict | None, *, key: str | None, now: datetime,
           incident_entries: list[dict] | None = None) -> None:
    """Raise AuthorizationError unless `auth` validly authorizes `auth_class` for exactly this declaration."""
    if auth_class not in AUTHORIZABLE:
        raise AuthorizationError(f"fail-closed: {auth_class!r} is not an authorizable stage class")
    if not key or len(key) < MIN_KEY_CHARS:
        raise AuthorizationError(f"fail-closed: no operator authorization secret ({KEY_ENV}) is available")
    if not isinstance(auth, dict) or auth.get("schema") != SCHEMA:
        raise AuthorizationError("fail-closed: no valid stage authorization was supplied")
    if not hmac.compare_digest(str(auth.get("signature", "")), _signature(auth, key)):
        raise AuthorizationError("fail-closed: stage authorization signature does not verify")
    if auth.get("declaration_identity_sha256") != declaration_identity(decl) or auth.get("campaign_id") != _campaign(decl):
        raise AuthorizationError("fail-closed: stage authorization is bound to a different declaration identity")
    if auth_class not in (auth.get("authorized_classes") or []):
        raise AuthorizationError(f"fail-closed: stage authorization does not authorize {auth_class!r}")
    if not auth.get("operator") or not auth.get("approval_ref"):
        raise AuthorizationError("fail-closed: stage authorization names no operator approval")
    try:
        issued = datetime.fromisoformat(auth["issued_utc"])
        expires = datetime.fromisoformat(auth["expires_utc"])
    except (KeyError, ValueError) as exc:
        raise AuthorizationError("fail-closed: stage authorization validity window is unreadable") from exc
    if not (timedelta(0) < expires - issued <= MAX_VALIDITY) or not (issued <= now < expires):
        raise AuthorizationError("fail-closed: stage authorization is expired, not yet valid, or has an invalid window")
    blocking = holdout_incident.affecting_incidents(decl, incident_entries)  # pending or consumed
    if blocking and not set(blocking) <= set(auth.get("acknowledged_incident_ids") or []):
        raise AuthorizationError(f"fail-closed: stage authorization does not acknowledge blocking incident(s) {blocking}")
    if auth_class == PROVIDER_FETCH:
        required = set((decl.get("data") or {}).get("required_fetch_acknowledgements") or [])
        if not required <= set(auth.get("acknowledged_data_boundaries") or []):
            raise AuthorizationError(f"fail-closed: the fetch authorization does not acknowledge data boundaries "
                                     f"{sorted(required - set(auth.get('acknowledged_data_boundaries') or []))}")
    if auth_class in INCIDENT_BLOCKED:
        holdout_incident.require_independence_clear(decl, auth_class, incident_entries)


def load_auth_file(path: str | os.PathLike | None) -> dict | None:
    if not path:
        return None
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def require_stage(decl: dict, stage: str, *, auth: dict | None = None, key: str | None = None,
                  now: datetime | None = None, incident_entries: list[dict] | None = None) -> dict | None:
    """Called at the top of every effectful runner stage, so a directly imported stage function is gated
    exactly like the CLI dispatcher. A frozen historical declaration and a read-only stage pass through (None).
    Returns the verified authorization, which carries the native-binary pin the stage may execute."""
    auth_class = STAGE_CLASS.get(stage)
    if auth_class is None:
        raise AuthorizationError(f"fail-closed: {stage!r} is not a known runner stage")
    if auth_class == READ_ONLY or is_frozen_historical(decl):
        return None
    auth = auth if auth is not None else load_auth_file(os.environ.get(AUTH_FILE_ENV))
    verify(decl, auth_class, auth, key=key if key is not None else os.environ.get(KEY_ENV),
           now=now or datetime.now(timezone.utc), incident_entries=incident_entries)
    return auth


def optional_authorization(decl: dict, auth_class: str, *, auth: dict | None = None, key: str | None = None,
                           now: datetime | None = None, incident_entries: list[dict] | None = None) -> dict | None:
    """The verified authorization for `auth_class`, or None when there is none. For a read-only stage that has an
    OPTIONAL authorized extension (running the native binary); it never weakens a mandatory gate."""
    try:
        auth = auth if auth is not None else load_auth_file(os.environ.get(AUTH_FILE_ENV))
        verify(decl, auth_class, auth, key=key if key is not None else os.environ.get(KEY_ENV),
               now=now or datetime.now(timezone.utc), incident_entries=incident_entries)
        return auth
    except AuthorizationError:
        return None


def verified_cli(auth: dict | None, cli: Path) -> Path:
    """The native binary a stage may execute: a regular file whose sha256 equals the authorization's signed
    `cli_sha256`. A different path, a swapped binary, an unpinned authorization or no authorization all refuse."""
    pin = (auth or {}).get("cli_sha256")
    if not isinstance(pin, str) or len(pin) != 64 or any(c not in "0123456789abcdef" for c in pin):
        raise AuthorizationError("fail-closed: no authorization pins a native binary (cli_sha256); nothing is executed")
    path = Path(cli)
    if not path.is_file():
        raise AuthorizationError(f"fail-closed: the native binary {path} is not a regular file")
    resolved = path.resolve()
    if hashlib.sha256(resolved.read_bytes()).hexdigest() != pin:
        raise AuthorizationError("fail-closed: the native binary differs from the one the authorization pins")
    return resolved
