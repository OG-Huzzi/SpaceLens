/**
 * Screen-facing UI facade.
 *
 * The primitives themselves live in `src/design/`. This module keeps the
 * higher-level compositions the screens share — page headers, section headings,
 * data wrappers, loading/error surfaces — and re-exports the vocabulary so a
 * screen never reaches for raw markup when a primitive exists.
 */

import type { ReactNode } from "react";
import type { ApiError, SafetyTier } from "../api";
import { formatBytes } from "../lib/format";
import { t } from "../i18n";
import { ShareBar, Skeleton, UnknownValue } from "../design";

export {
  Button,
  CoverageBanner,
  EmptyState,
  Field,
  KeyHint,
  Metric,
  Panel,
  ShareBar,
  Skeleton,
  Surface,
  Tier,
  UnknownValue,
  SpaceMap
} from "../design";
export type { CoverageContext, SpaceMapItem } from "../design";

/* ------------------------------------------------------------------------- */
/* Page chrome                                                                */
/* ------------------------------------------------------------------------- */

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
        <p className="cs-eyebrow">{eyebrow}</p>
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
      <div className="section-heading__copy">
        <Heading id={id}>{title}</Heading>
        {description ? <p>{description}</p> : null}
      </div>
      {aside ? <div className="section-heading__aside">{aside}</div> : null}
    </div>
  );
}

/* ------------------------------------------------------------------------- */
/* Data wrappers                                                              */
/* ------------------------------------------------------------------------- */

export function PartialTag({ label }: { label?: string }) {
  return (
    <span className="cs-partial-tag" title={t("state.partialTag.title")}>
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
    <span className={`cs-data${className ? ` ${className}` : ""}`}>
      {children}
      {partial ? <PartialTag /> : null}
    </span>
  );
}

/**
 * A byte value that refuses to turn "not measured" into "0 B".
 * Known values are mono and tabular; null is the dashed Unknown state.
 */
export function Bytes({ value, partial = false }: { value: number | null; partial?: boolean }) {
  if (value === null) return <UnknownValue />;
  return <DataValue partial={partial}>{formatBytes(value)}</DataValue>;
}

const TIER_LABEL: Record<SafetyTier, "tier.safe" | "tier.review" | "tier.offLimits"> = {
  SAFE: "tier.safe",
  REVIEW: "tier.review",
  OFF_LIMITS: "tier.offLimits"
};

export function TierBadge({ tier }: { tier: SafetyTier }) {
  return (
    <span className={`cs-tier cs-tier--${tier.toLowerCase().replace("_", "-")}`}>
      <span className="cs-tier__mark" aria-hidden="true" />
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
  return <span className={`cs-pill cs-pill--${tone}`}>{children}</span>;
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
  return <ShareBar share={share} label={label} partial={partial} />;
}

/* ------------------------------------------------------------------------- */
/* Screen states                                                              */
/* ------------------------------------------------------------------------- */

export type LoadingLayout =
  | "home"
  | "table"
  | "cards"
  | "timeline"
  | "drives"
  | "settings"
  | "panels";

function LoadingShapes({ layout }: { layout: LoadingLayout }) {
  if (layout === "home") {
    return (
      <div className="cs-loading__home">
        <div className="cs-loading__map">
          <Skeleton lines={1} height="100%" />
        </div>
        <div className="cs-loading__rail">
          <Skeleton lines={4} height="var(--cs-space-4)" />
          <Skeleton lines={3} height="var(--cs-space-3)" />
        </div>
        <div className="cs-loading__rows">
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} lines={1} height="var(--cs-space-5)" />
          ))}
        </div>
      </div>
    );
  }
  if (layout === "table" || layout === "panels") {
    return (
      <div className="cs-loading__table">
        <Skeleton lines={1} height="var(--cs-space-6)" />
        {Array.from({ length: 6 }, (_, index) => (
          <Skeleton key={index} lines={1} height="var(--cs-row-height)" />
        ))}
      </div>
    );
  }
  if (layout === "timeline") {
    return (
      <div className="cs-loading__timeline">
        {Array.from({ length: 3 }, (_, index) => (
          <Skeleton key={index} lines={3} height="var(--cs-space-4)" />
        ))}
      </div>
    );
  }
  if (layout === "drives" || layout === "settings" || layout === "cards") {
    return (
      <div className="cs-loading__cards">
        {Array.from({ length: layout === "drives" ? 2 : 3 }, (_, index) => (
          <Skeleton key={index} lines={5} height="var(--cs-space-4)" />
        ))}
      </div>
    );
  }
  return <Skeleton lines={4} />;
}

export function ScreenLoading({
  eyebrow,
  title,
  description,
  layout = "panels"
}: {
  eyebrow: string;
  title: string;
  description: string;
  layout?: LoadingLayout;
}) {
  return (
    <div className="screen" aria-busy="true">
      <PageHeader eyebrow={eyebrow} title={title} description={description} />
      <section className={`cs-loading cs-loading--${layout}`} role="status" aria-live="polite">
        <span className="visually-hidden">
          {t("state.loading.heading")} {t("state.loading.body")}
        </span>
        <LoadingShapes layout={layout} />
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
      <section className="cs-error" role="alert">
        <p className="cs-eyebrow">{t("state.error.eyebrow")}</p>
        <h2>{error.message}</h2>
        {error.detail ? <p>{error.detail}</p> : null}
        <p className="cs-error__code">
          {t("state.error.reference")} <code>{error.code}</code>
        </p>
        <p className="cs-error__body">{t("state.error.nothingChanged")}</p>
        <a className="cs-link" href="#home">
          {t("state.error.returnHome")}
        </a>
      </section>
    </div>
  );
}

export function InlineError({ error }: { error: ApiError }) {
  return (
    <div className="cs-inline cs-inline--error" role="alert">
      <strong>{error.message}</strong>
      {error.detail ? <p>{error.detail}</p> : null}
      <span className="cs-inline__code">
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
  return <div className={`cs-inline cs-inline--${tone}`}>{children}</div>;
}
