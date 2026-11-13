import { useId, type ReactNode } from "react";
import type { ApiError } from "../api";
import { t } from "../i18n";
import {
  InlineError,
  SectionHeading,
  StatusPill
} from "../components/ui";

/**
 * Shared chrome for the read-only report panels.
 *
 * Every panel on Home/History is an explanation surface, never an action
 * surface. The "Read-only" pill is part of the chrome rather than one panel's
 * copy because the promise is the same for all four: a startup item cannot be
 * disabled here, DNS cannot be switched here, a block list cannot be applied
 * here, and a privacy finding cannot be deleted here.
 *
 * The unavailable state is deliberately separate from the error state. Codes
 * such as SESSION_UNAVAILABLE and HISTORY_UNAVAILABLE are the engine saying
 * "this build has no inventory for that record yet". Rendering them through
 * the generic error panel would read as a failure, and rendering the empty or
 * zero-data success state would claim the record is empty when it was never
 * read. Neither is true, so they get their own calm state with the reason.
 */

const UNAVAILABLE_CODES: ReadonlySet<string> = new Set([
  "SESSION_UNAVAILABLE",
  "HISTORY_UNAVAILABLE",
  "NOT_IMPLEMENTED_IN_THIS_BUILD",
  "UNSUPPORTED_IN_THIS_BUILD"
]);

export function isUnavailableError(error: ApiError): boolean {
  return UNAVAILABLE_CODES.has(error.code);
}

export function PanelFrame({
  id,
  title,
  description,
  children
}: {
  id: string;
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="panel panel--readonly" aria-labelledby={id}>
      <SectionHeading
        id={id}
        title={title}
        description={description}
        aside={<StatusPill tone="quiet">{t("panel.readOnly")}</StatusPill>}
      />
      {children}
    </section>
  );
}

export function PanelLoading({ label }: { label: string }) {
  return (
    <div
      className="state-panel state-panel--loading"
      role="status"
      aria-live="polite"
      aria-label={t("state.loading.label")}
    >
      <div className="state-panel__heading">
        <span className="spinner" aria-hidden="true" />
        <h2>{label}</h2>
      </div>
      <p>{t("panel.loading.body")}</p>
    </div>
  );
}

export function PanelUnavailable({ error }: { error: ApiError }) {
  const headingId = useId();
  return (
    <div className="state-panel" role="status" aria-labelledby={headingId}>
      <p className="eyebrow">{t("panel.unavailable.eyebrow")}</p>
      <h2 id={headingId}>{t("panel.unavailable.title")}</h2>
      <p>
        <strong>{t("panel.unavailable.reason")}</strong> {error.message}
      </p>
      {error.detail ? <p>{error.detail}</p> : null}
      <p className="state-panel__code">
        {t("state.error.reference")} <code>{error.code}</code>
      </p>
      <p className="microcopy">{t("panel.unavailable.body")}</p>
    </div>
  );
}

/**
 * One decision point for the two error shapes. A known "not in this build"
 * code gets the calm state; everything else keeps the standard alert-style
 * inline error, because a real read failure must not look like a feature.
 */
export function PanelFailure({ error }: { error: ApiError }) {
  return isUnavailableError(error) ? (
    <PanelUnavailable error={error} />
  ) : (
    <InlineError error={error} />
  );
}
