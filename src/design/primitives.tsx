/**
 * CoreSight primitives.
 *
 * The names and props in docs/DESIGN_SYSTEM.md are binding. Everything a screen
 * renders should come from this module (directly or through the ui.tsx facade)
 * so that the token vocabulary stays enforceable in one place.
 */

import { useId, type ButtonHTMLAttributes, type ReactNode } from "react";
import type { DataCoverage, SafetyTier } from "../api";
import { formatPercent } from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";

/* ------------------------------------------------------------------------- */
/* Surface and Panel                                                          */
/* ------------------------------------------------------------------------- */

export type SurfaceTone = "default" | "raised" | "sunken";

export function Surface({
  tone = "default",
  padded = false,
  className,
  children
}: {
  tone?: SurfaceTone;
  padded?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const classes = [
    "cs-surface",
    `cs-surface--${tone}`,
    padded ? "cs-surface--padded" : "",
    className ?? ""
  ]
    .filter(Boolean)
    .join(" ");
  return <div className={classes}>{children}</div>;
}

export function Panel({
  title,
  description,
  actions,
  children,
  id,
  level = 2,
  className
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  id?: string;
  level?: 2 | 3;
  className?: string;
}) {
  const autoId = useId();
  const headingId = id ?? autoId;
  const Heading = level === 2 ? "h2" : "h3";
  return (
    <section
      className={`cs-panel${className ? ` ${className}` : ""}`}
      aria-labelledby={headingId}
    >
      <header className="cs-panel__head">
        <div className="cs-panel__heading">
          <Heading className="cs-panel__title" id={headingId}>
            {title}
          </Heading>
          {description ? <p className="cs-panel__description">{description}</p> : null}
        </div>
        {actions ? <div className="cs-panel__actions">{actions}</div> : null}
      </header>
      <div className="cs-panel__body">{children}</div>
    </section>
  );
}

/* ------------------------------------------------------------------------- */
/* Data display                                                               */
/* ------------------------------------------------------------------------- */

export function UnknownValue({ label }: { label?: string }) {
  return (
    <span className="cs-unknown" data-cs-state="unknown">
      {label ?? t("state.unknown")}
    </span>
  );
}

export function Metric({
  label,
  value,
  unit,
  unknown = false,
  delta,
  hint
}: {
  label: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  unknown?: boolean;
  delta?: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <div className={`cs-metric${unknown ? " cs-metric--unknown" : ""}`}>
      <span className="cs-metric__label">{label}</span>
      <span className="cs-metric__value cs-num">
        {unknown ? <UnknownValue /> : value}
        {!unknown && unit ? <span className="cs-metric__unit">{unit}</span> : null}
      </span>
      {delta ? <span className="cs-metric__delta cs-num">{delta}</span> : null}
      {hint ? <span className="cs-metric__hint">{hint}</span> : null}
    </div>
  );
}

export function ShareBar({
  share,
  label,
  tone = "accent",
  partial = false
}: {
  share: number | null;
  label: string;
  tone?: "accent" | "safe" | "review" | "offlimits" | "muted";
  partial?: boolean;
}) {
  const clamped = share === null ? null : Math.min(1, Math.max(0, share));
  const ariaLabel =
    share === null
      ? t("state.sizeBar.unknown", { label })
      : `${t("state.sizeBar.share", { label, percent: formatPercent(share) })}${
          partial ? t("state.sizeBar.partialSuffix") : ""
        }`;
  return (
    <span
      className={`cs-sharebar cs-sharebar--${tone}${
        share === null ? " cs-sharebar--unknown" : ""
      }`}
      role="img"
      aria-label={ariaLabel}
    >
      {clamped === null ? (
        <span className="cs-sharebar__unknown" aria-hidden="true" />
      ) : (
        <span
          className="cs-sharebar__fill"
          style={{ width: `${clamped * 100}%` }}
          aria-hidden="true"
        />
      )}
    </span>
  );
}

/* ------------------------------------------------------------------------- */
/* Safety vocabulary                                                          */
/* ------------------------------------------------------------------------- */

const TIER_LABEL: Record<SafetyTier, MessageKey> = {
  SAFE: "tier.safe",
  REVIEW: "tier.review",
  OFF_LIMITS: "tier.offLimits"
};

export function Tier({ tier, children }: { tier: SafetyTier; children?: ReactNode }) {
  return (
    <span className={`cs-tier cs-tier--${tier.toLowerCase().replace("_", "-")}`}>
      <span className="cs-tier__mark" aria-hidden="true" />
      {children ?? t(TIER_LABEL[tier])}
    </span>
  );
}

/* ------------------------------------------------------------------------- */
/* Coverage and states                                                        */
/* ------------------------------------------------------------------------- */

export type CoverageContext =
  | "scan"
  | "storageScan"
  | "cleanupPreview"
  | "duplicateAnalysis"
  | "largeFileResult";

const INCOMPLETE_HEADING: Record<CoverageContext, MessageKey> = {
  scan: "state.coverage.incomplete.scan",
  storageScan: "state.coverage.incomplete.storageScan",
  cleanupPreview: "state.coverage.incomplete.cleanupPreview",
  duplicateAnalysis: "state.coverage.incomplete.duplicateAnalysis",
  largeFileResult: "state.coverage.incomplete.largeFileResult"
};

function counted(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.entry.one", "unit.entry.other");
}

export function CoverageBanner({
  complete,
  erroredEntries,
  entriesWithoutSize,
  context,
  coverage,
  className
}: {
  complete: boolean;
  erroredEntries?: number | null;
  entriesWithoutSize?: number | null;
  context: CoverageContext;
  coverage?: DataCoverage;
  className?: string;
}) {
  const headingId = useId();
  const note = coverage?.note ?? null;
  const excludedAreas = coverage?.excludedAreas ?? [];
  if (complete && note === null && excludedAreas.length === 0) return null;

  const unreadable = counted(erroredEntries ?? coverage?.unreadableEntries ?? null);
  const unknownSizes = counted(
    entriesWithoutSize ?? coverage?.unknownSizeEntries ?? null
  );

  return (
    <aside
      className={`cs-coverage${className ? ` ${className}` : ""}`}
      aria-labelledby={headingId}
      role="note"
    >
      <span className="cs-coverage__mark" aria-hidden="true">
        !
      </span>
      <div className="cs-coverage__body">
        <h2 className="cs-coverage__title" id={headingId}>
          {complete ? t("state.coverage.outsideHeading") : t(INCOMPLETE_HEADING[context])}
        </h2>
        <p className="cs-coverage__lead">
          {complete ? t("state.coverage.outsideBody") : t("state.coverage.incompleteBody")}
        </p>
        <ul className="cs-coverage__facts">
          <li aria-label={t("state.coverage.unreadableAria", { value: unreadable })}>
            <strong>{t("state.coverage.unreadableLabel")}</strong>
            <span className="cs-num">{unreadable}</span>
          </li>
          <li aria-label={t("state.coverage.unknownSizesAria", { value: unknownSizes })}>
            <strong>{t("state.coverage.unknownSizesLabel")}</strong>
            <span className="cs-num">{unknownSizes}</span>
          </li>
        </ul>
        {excludedAreas.length > 0 ? (
          <p className="cs-coverage__excluded">
            <strong>{t("state.coverage.notIncluded")}</strong> {excludedAreas.join(" · ")}
          </p>
        ) : null}
        {note ? <p className="cs-coverage__note">{note}</p> : null}
      </div>
    </aside>
  );
}

export function EmptyState({
  title,
  body,
  next
}: {
  title: string;
  body?: string;
  next?: string;
}) {
  return (
    <section className="cs-empty" role="status">
      <span className="cs-empty__rule" aria-hidden="true" />
      <h2 className="cs-empty__title">{title}</h2>
      {body ? <p className="cs-empty__body">{body}</p> : null}
      {next ? <p className="cs-empty__next">{next}</p> : null}
    </section>
  );
}

export function Skeleton({ lines = 3, height }: { lines?: number; height?: number | string }) {
  return (
    <span className="cs-skeleton" aria-hidden="true">
      {Array.from({ length: lines }, (_, index) => (
        <span
          className="cs-skeleton__line"
          key={index}
          style={{
            height: height ?? "var(--cs-text-base)",
            width: index === lines - 1 && lines > 1 ? "62%" : "100%"
          }}
        />
      ))}
    </span>
  );
}

/* ------------------------------------------------------------------------- */
/* Controls                                                                   */
/* ------------------------------------------------------------------------- */

export interface ButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className" | "onClick" | "type"> {
  variant: "primary" | "secondary" | "ghost" | "danger";
  disabled?: boolean;
  onClick?: () => void;
  className?: string;
  children: ReactNode;
}

export function Button({
  variant,
  disabled,
  onClick,
  className,
  children,
  ...rest
}: ButtonProps) {
  return (
    <button
      {...rest}
      type="button"
      className={`cs-button cs-button--${variant}${className ? ` ${className}` : ""}`}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

export function KeyHint({ keys }: { keys: string[] }) {
  return (
    <span className="cs-keyhint">
      {keys.map((key) => (
        <kbd className="cs-keyhint__key" key={key}>
          {key}
        </kbd>
      ))}
    </span>
  );
}

export function Field({
  label,
  hint,
  error,
  children,
  htmlFor,
  hintId,
  className
}: {
  label: string;
  hint?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
  htmlFor?: string;
  hintId?: string;
  className?: string;
}) {
  return (
    <div className={`cs-field${error ? " cs-field--error" : ""}${className ? ` ${className}` : ""}`}>
      <label className="cs-field__label" htmlFor={htmlFor}>
        {label}
      </label>
      <div className="cs-field__control">{children}</div>
      {hint ? (
        <p className="cs-field__hint" id={hintId}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p className="cs-field__error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
