import { useId, type CSSProperties, type ReactNode } from "react";
import type { ApiError, DataCoverage, SafetyTier } from "../api";
import { formatCount, formatPercent } from "../lib/format";

export interface PageHeaderProps {
  eyebrow: string;
  title: string;
  description: string;
  aside?: ReactNode;
}

export function PageHeader({ eyebrow, title, description, aside }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header__copy">
        <p className="eyebrow">{eyebrow}</p>
        <h1 tabIndex={-1}>{title}</h1>
        <p className="page-header__intro">{description}</p>
      </div>
      {aside ? <div className="page-header__aside">{aside}</div> : null}
    </header>
  );
}

export interface SectionHeadingProps {
  title: string;
  description?: string;
  aside?: ReactNode;
  level?: 2 | 3;
  id?: string;
}

export function SectionHeading({
  title,
  description,
  aside,
  level = 2,
  id
}: SectionHeadingProps) {
  const Heading = level === 2 ? "h2" : "h3";
  return (
    <div className="section-heading">
      <div>
        <Heading id={id}>{title}</Heading>
        {description ? <p>{description}</p> : null}
      </div>
      {aside ? <div className="section-heading__aside">{aside}</div> : null}
    </div>
  );
}

export function PartialTag({ label = "partial" }: { label?: string }) {
  return (
    <span
      className="quality-tag"
      title="This figure comes from a partial source; it is not a complete total."
    >
      {label}
    </span>
  );
}

export function DataValue({
  children,
  partial = false,
  className
}: {
  children: ReactNode;
  partial?: boolean;
  className?: string;
}) {
  return (
    <span className={`data-value${className ? ` ${className}` : ""}`}>
      {children}
      {partial ? <PartialTag /> : null}
    </span>
  );
}

export function CoverageBanner({
  coverage,
  context = "scan"
}: {
  coverage: DataCoverage;
  context?: string;
}) {
  const headingId = useId();
  if (
    coverage.completeness === "complete" &&
    coverage.note === null &&
    coverage.excludedAreas.length === 0
  ) {
    return null;
  }

  const incomplete = coverage.completeness !== "complete";
  return (
    <aside className="notice notice--partial" aria-labelledby={headingId} role="note">
      <span className="notice__mark" aria-hidden="true">
        !
      </span>
      <div className="notice__body">
        <h2 id={headingId}>
          {incomplete
            ? `This ${context} is incomplete`
            : "Some entries are outside this result"}
        </h2>
        <p>
          {incomplete
            ? "Totals here are lower bounds, not complete totals. Aggregate figures from this source carry a partial label."
            : "The figures below describe only the entries the engine could evaluate."}
        </p>
        <ul className="notice__facts">
          <li
            aria-label={`Unreadable entries: ${
              coverage.unreadableEntries === null
                ? "Unknown"
                : formatCount(coverage.unreadableEntries, "entry", "entries")
            }`}
          >
            <strong>Unreadable entries:</strong>{" "}
            {coverage.unreadableEntries === null
              ? "Unknown"
              : formatCount(coverage.unreadableEntries, "entry", "entries")}
          </li>
          <li
            aria-label={`Unknown sizes: ${
              coverage.unknownSizeEntries === null
                ? "Unknown"
                : formatCount(coverage.unknownSizeEntries, "entry", "entries")
            }`}
          >
            <strong>Unknown sizes:</strong>{" "}
            {coverage.unknownSizeEntries === null
              ? "Unknown"
              : formatCount(coverage.unknownSizeEntries, "entry", "entries")}
          </li>
        </ul>
        {coverage.excludedAreas.length > 0 ? (
          <p className="notice__excluded">
            <strong>Not included:</strong> {coverage.excludedAreas.join(" · ")}
          </p>
        ) : null}
        {coverage.note ? <p className="notice__note">{coverage.note}</p> : null}
      </div>
    </aside>
  );
}

const TIER_LABEL: Record<SafetyTier, string> = {
  SAFE: "Safe",
  REVIEW: "Review",
  OFF_LIMITS: "Off-limits"
};

export function TierBadge({ tier }: { tier: SafetyTier }) {
  return (
    <span className={`tier-badge tier-badge--${tier.toLowerCase().replace("_", "-")}`}>
      <span className="tier-badge__mark" aria-hidden="true" />
      {TIER_LABEL[tier]}
    </span>
  );
}

export type PillTone = "neutral" | "positive" | "attention" | "quiet";

export function StatusPill({
  children,
  tone = "neutral"
}: {
  children: ReactNode;
  tone?: PillTone;
}) {
  return <span className={`status-pill status-pill--${tone}`}>{children}</span>;
}

export function SizeBar({
  share,
  label,
  partial = false
}: {
  share: number | null;
  label: string;
  partial?: boolean;
}) {
  const width = share === null ? undefined : `${Math.min(100, Math.max(0, share * 100))}%`;
  const style: CSSProperties | undefined = width ? { width } : undefined;
  const ariaLabel =
    share === null
      ? `${label}: share unknown`
      : `${label}: ${formatPercent(share)}${partial ? ", from a partial source" : ""}`;

  return (
    <span
      className={`size-bar${share === null ? " size-bar--unknown" : ""}`}
      role="img"
      aria-label={ariaLabel}
    >
      {share === null ? (
        <span className="size-bar__unknown" aria-hidden="true" />
      ) : (
        <span className="size-bar__fill" style={style} aria-hidden="true" />
      )}
    </span>
  );
}

export function ScreenLoading({
  eyebrow,
  title,
  description
}: {
  eyebrow: string;
  title: string;
  description: string;
}) {
  return (
    <div className="screen" aria-busy="true">
      <PageHeader eyebrow={eyebrow} title={title} description={description} />
      <section className="state-panel state-panel--loading" role="status" aria-live="polite">
        <div className="state-panel__heading">
          <span className="spinner" aria-hidden="true" />
          <h2>Reading the local snapshot</h2>
        </div>
        <p>Large scans can take a moment. This screen will update when the data is ready.</p>
        <div className="skeleton-stack" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
        <span className="visually-hidden">Loading</span>
      </section>
    </div>
  );
}

export function ScreenError({
  eyebrow,
  title,
  description,
  error
}: {
  eyebrow: string;
  title: string;
  description: string;
  error: ApiError;
}) {
  return (
    <div className="screen">
      <PageHeader eyebrow={eyebrow} title={title} description={description} />
      <section className="state-panel state-panel--error" role="alert">
        <p className="eyebrow">Could not load this screen</p>
        <h2>{error.message}</h2>
        {error.detail ? <p>{error.detail}</p> : null}
        <p className="state-panel__code">
          Reference <code>{error.code}</code>
        </p>
        <p>
          Nothing was changed. Leave this screen and open it again; if the snapshot is still
          unavailable, restart CoreSight.
        </p>
        <a className="text-link" href="#home">
          Return to Home
        </a>
      </section>
    </div>
  );
}

export function EmptyState({
  title,
  description,
  nextStep
}: {
  title: string;
  description: string;
  nextStep: string;
}) {
  return (
    <section className="empty-state" role="status">
      <div className="empty-state__rule" aria-hidden="true" />
      <h2>{title}</h2>
      <p>{description}</p>
      <p className="empty-state__next">{nextStep}</p>
    </section>
  );
}

export function InlineError({ error }: { error: ApiError }) {
  return (
    <div className="inline-message inline-message--error" role="alert">
      <strong>{error.message}</strong>
      {error.detail ? <p>{error.detail}</p> : null}
      <span className="inline-message__code">Reference {error.code}</span>
    </div>
  );
}

export function InlineNotice({
  children,
  tone = "quiet"
}: {
  children: ReactNode;
  tone?: "quiet" | "attention";
}) {
  return <div className={`inline-message inline-message--${tone}`}>{children}</div>;
}
