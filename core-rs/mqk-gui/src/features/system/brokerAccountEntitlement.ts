// core-rs/mqk-gui/src/features/system/brokerAccountEntitlement.ts
//
// Pure helpers for the preflight `broker_account_entitlement` and
// `broker_start_blockers` fields: structural validation (fail closed on a
// malformed body) and operator presentation. The daemon is the only authority:
// nothing here infers entitlement, and a state other than "entitled" is never
// presented as ready. Fixtures proving this module say nothing about a real
// provider account.

import type {
  BrokerAccountEntitlement,
  BrokerAccountEntitlementState,
  BrokerStartBlocker,
  PreflightStatus,
} from "./types.ts";

const ENTITLEMENT_STATES: readonly BrokerAccountEntitlementState[] = [
  "entitled",
  "denied",
  "stale",
  "unbound",
  "unknown",
  "not_observed",
];

const isNullableString = (v: unknown): boolean => v === null || typeof v === "string";

export function isBrokerAccountEntitlement(v: unknown): v is BrokerAccountEntitlement {
  if (!v || typeof v !== "object") return false;
  const e = v as Record<string, unknown>;
  return (
    typeof e["state"] === "string" &&
    (ENTITLEMENT_STATES as readonly string[]).includes(e["state"] as string) &&
    typeof e["asset_class"] === "string" &&
    isNullableString(e["code"]) &&
    isNullableString(e["detail"]) &&
    isNullableString(e["provider_account_id"]) &&
    isNullableString(e["observed_at_utc"])
  );
}

export function isBrokerStartBlocker(v: unknown): v is BrokerStartBlocker {
  if (!v || typeof v !== "object") return false;
  const b = v as Record<string, unknown>;
  return typeof b["code"] === "string" && typeof b["message"] === "string";
}

/**
 * Both fields are additive and optional (absent on older daemons, `null` when
 * the broker has no provider account). When PRESENT they must be well-formed:
 * a body whose entitlement object is malformed is a contract failure, not an
 * absent field, so the caller falls back to the unavailable preflight.
 */
export function brokerReadinessFieldsStructurallyValid(p: Record<string, unknown>): boolean {
  const ent = p["broker_account_entitlement"];
  const blockers = p["broker_start_blockers"];
  const entOk = ent === undefined || ent === null || isBrokerAccountEntitlement(ent);
  const blockersOk =
    blockers === undefined || (Array.isArray(blockers) && blockers.every(isBrokerStartBlocker));
  return entOk && blockersOk;
}

export type BrokerCheckTone = "ok" | "blocked" | "warning" | "unknown";

export interface BrokerCheck {
  /** Stable key: entitlement | identity | endpoint | credentials | start_blocker */
  kind: "entitlement" | "identity" | "endpoint" | "credentials" | "start_blocker";
  tone: BrokerCheckTone;
  title: string;
  detail: string;
  /** Structured daemon code, retained verbatim. */
  code: string | null;
}

const IDENTITY_CODES = new Set([
  "account_identity_drift",
  "account_identity_unavailable",
  "runtime.start_refused.broker_account_identity_conflict",
  "runtime.start_refused.broker_account_unproven",
]);
const ENDPOINT_CODES = new Set(["runtime.start_refused.alpaca_paper_base_url_not_paper"]);
const CREDENTIAL_CODES = new Set([
  "runtime.start_refused.alpaca_creds_missing",
  "runtime.start_refused.alpaca_creds_malformed",
]);

function entitlementCheck(e: BrokerAccountEntitlement): BrokerCheck {
  const observed = e.observed_at_utc ? ` Last observed ${e.observed_at_utc}.` : "";
  const account = e.provider_account_id ? ` Account ${e.provider_account_id}.` : "";
  const code = e.code ?? null;
  switch (e.state) {
    case "entitled":
      return {
        kind: "entitlement",
        tone: "ok",
        title: "Broker account entitled",
        detail: `Fresh daemon evidence is bound to this run.${account}${observed}`,
        code,
      };
    case "denied":
      if (code !== null && IDENTITY_CODES.has(code)) {
        return {
          kind: "identity",
          tone: "blocked",
          title: "Broker account identity refused",
          detail: `${e.detail ?? "Observed account does not match the account bound to this run."}${account}`,
          code,
        };
      }
      return {
        kind: "entitlement",
        tone: "blocked",
        title: "Broker account denied",
        detail: `${e.detail ?? "The provider reports the account cannot trade."}${account}`,
        code,
      };
    case "stale":
      return {
        kind: "entitlement",
        tone: "warning",
        title: "Broker account evidence stale",
        detail: `Not order-authorized until a fresh observation exists.${observed}`,
        code,
      };
    case "unbound":
      return {
        kind: "entitlement",
        tone: "warning",
        title: "Broker account not bound to a run",
        detail: `Evidence exists but no validated run/account binding; orders are refused until a start binds the account.${account}${observed}`,
        code,
      };
    case "not_observed":
      return {
        kind: "entitlement",
        tone: "unknown",
        title: "Broker account not yet observed",
        detail: "Checked when a run starts; not order-authorized now.",
        code,
      };
    default:
      return {
        kind: "entitlement",
        tone: "unknown",
        title: "Broker account evidence unknown",
        detail: `${e.detail ?? "Provider fields were unavailable or malformed."} Not order-authorized.`,
        code,
      };
  }
}

function blockerCheck(b: BrokerStartBlocker): BrokerCheck {
  const kind = IDENTITY_CODES.has(b.code)
    ? "identity"
    : ENDPOINT_CODES.has(b.code)
      ? "endpoint"
      : CREDENTIAL_CODES.has(b.code)
        ? "credentials"
        : "start_blocker";
  const title =
    kind === "identity"
      ? "Broker account identity refused"
      : kind === "endpoint"
        ? "Broker endpoint refused"
        : kind === "credentials"
          ? "Broker credentials not usable"
          : "Broker start refused";
  return { kind, tone: "blocked", title, detail: b.message, code: b.code };
}

/**
 * Ordered checks for the broker account surface. Absent/null entitlement means
 * the selected broker has no provider account: no entitlement check is shown.
 * Start blockers whose code duplicates the entitlement code are not repeated.
 */
export function brokerAccountChecks(
  preflight: Pick<PreflightStatus, "broker_account_entitlement" | "broker_start_blockers">,
): BrokerCheck[] {
  const checks: BrokerCheck[] = [];
  const ent = preflight.broker_account_entitlement ?? null;
  const entCode = ent?.code ?? null;
  if (ent) checks.push(entitlementCheck(ent));
  for (const b of preflight.broker_start_blockers ?? []) {
    if (entCode !== null && b.code === entCode) continue;
    checks.push(blockerCheck(b));
  }
  return checks;
}
