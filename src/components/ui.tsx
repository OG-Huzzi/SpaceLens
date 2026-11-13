import { useId, type CSSProperties, type ReactNode } from "react";
import type { ApiError, DataCoverage, SafetyTier } from "../api";
import { formatPercent } from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";

/**
 * Shared state surfaces. Every fixed string here is localised; the strings a
 * screen passes in as props (titles, descriptions) are still the screen's own
 * copy and remain English in the screens that have not been localised yet.
 */

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

export function PartialTag({ label }: { label?: string }) {
  return (
    <span className="quality-tag" title={t("state.partialTag.title")}>
      {label ?? t("state.partialTag.label")}
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

/** Each context renders its own heading so translations keep correct grammar. */
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

function coveredCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.entry.one", "unit.entry.other");
}

export function CoverageBanner({
  coverage,
  context = "scan"
}: {
  coverage: DataCoverage;
  context?: CoverageContext;
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
  const unreadable = coveredCount(coverage.unreadableEntries);
  const unknownSizes = coveredCount(coverage.unknownSizeEntries);

  return (
    <aside className="notice notice--partial" aria-labelledby={headingId} role="note">
      <span className="notice__mark" aria-hidden="true">
        !
      </span>
      <div className="notice__body">
        <h2 id={headingId}>
          {incomplete
            ? t(INCOMPLETE_HEADING[context])
            : t("state.coverage.outsideHeading")}
        </h2>
        <p>
          {incomplete ? t("state.coverage.incompleteBody") : t("state.coverage.outsideBody")}
        </p>
        <ul className="notice__facts">
          <li aria-label={t("state.coverage.unreadableAria", { value: unreadable })}>
            <strong>{t("state.coverage.unreadableLabel")}</strong> {unreadable}
          </li>
          <li aria-label={t("state.coverage.unknownSizesAria", { value: unknownSizes })}>
            <strong>{t("state.coverage.unknownSizesLabel")}</strong> {unknownSizes}
          </li>
        </ul>
        {coverage.excludedAreas.length > 0 ? (
          <p className="notice__excluded">
            <strong>{t("state.coverage.notIncluded")}</strong>{" "}
            {coverage.excludedAreas.join(" · ")}
          </p>
        ) : null}
        {coverage.note ? <p className="notice__note">{coverage.note}</p> : null}
      </div>
    </aside>
  );
}

const TIER_LABEL: Record<SafetyTier, MessageKey> = {
  SAFE: "tier.safe",
  REVIEW: "tier.review",
  OFF_LIMITS: "tier.offLimits"
};

export function TierBadge({ tier }: { tier: SafetyTier }) {
  return (
    <span className={`tier-badge tier-badge--${tier.toLowerCase().replace("_", "-")}`}>
      <span className="tier-badge__mark" aria-hidden="true" />
      {t(TIER_LABEL[tier])}
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
      ? t("state.sizeBar.unknown", { label })
      : `${t("state.sizeBar.share", { label, percent: formatPercent(share) })}${
          partial ? t("state.sizeBar.partialSuffix") : ""
        }`;

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
          <h2>{t("state.loading.heading")}</h2>
        </div>
        <p>{t("state.loading.body")}</p>
        <div className="skeleton-stack" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
        <span className="visually-hidden">{t("state.loading.label")}</span>
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
        <p className="eyebrow">{t("state.error.eyebrow")}</p>
        <h2>{error.message}</h2>
        {error.detail ? <p>{error.detail}</p> : null}
        <p className="state-panel__code">
          {t("state.error.reference")} <code>{error.code}</code>
        </p>
        <p>{t("state.error.nothingChanged")}</p>
        <a className="text-link" href="#home">
          {t("state.error.returnHome")}
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
      <span className="inline-message__code">
        {t("state.error.reference")} {error.code}
      </span>
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
