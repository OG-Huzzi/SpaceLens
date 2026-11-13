import type { CoreSightApi, DataCoverage, StartupItem, StartupTier } from "../api";
import { useApiData } from "../hooks/useApiData";
import { t, tCount, type MessageKey } from "../i18n";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineNotice,
  SectionHeading,
  StatusPill,
  type PillTone
} from "../components/ui";
import { PanelFailure, PanelFrame, PanelLoading } from "./PanelFrame";

const STARTUP_TIER: Record<StartupTier, { label: MessageKey; tone: PillTone }> = {
  SAFE_TO_DISABLE: { label: "startup.tier.safeToDisable", tone: "positive" },
  REVIEW_FIRST: { label: "startup.tier.reviewFirst", tone: "attention" },
  OFF_LIMITS: { label: "startup.tier.offLimits", tone: "attention" }
};

function startupCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.startupItem.one", "unit.startupItem.other");
}

function StartupItemCard({ item }: { item: StartupItem }) {
  const tier = STARTUP_TIER[item.tier];
  return (
    <li className="cleanup-card">
      <div className="cleanup-card__header">
        <div className="cleanup-card__title">
          <StatusPill tone={tier.tone}>{t(tier.label)}</StatusPill>
          <h3>{item.name}</h3>
          <p>{t("startup.item.source", { source: item.sourceLabel })}</p>
        </div>
        <StatusPill tone={item.enabled ? "positive" : "quiet"}>
          {t(item.enabled ? "startup.item.enabled" : "startup.item.disabled")}
        </StatusPill>
      </div>

      <dl className="summary-facts">
        <div>
          <dt>{t("startup.item.owner")}</dt>
          <dd>{item.owner ?? t("startup.item.ownerUnknown")}</dd>
        </div>
        <div>
          <dt>{t("startup.item.command")}</dt>
          <dd>{item.commandLabel ?? t("state.unknown")}</dd>
        </div>
        <div>
          <dt>{t("startup.item.sourceFamily")}</dt>
          <dd>{item.sourceLabel}</dd>
        </div>
      </dl>

      <div className="cleanup-card__detail">
        <h4>{t("startup.item.why")}</h4>
        <p>{item.why}</p>
        <p className="microcopy">{t("startup.item.explanationOnly")}</p>
      </div>
    </li>
  );
}

function StartupOffLimits({
  items,
  partial
}: {
  items: StartupItem[];
  partial: boolean;
}) {
  return (
    <section
      className="panel panel--offlimits"
      aria-labelledby="startup-offlimits-heading"
    >
      <SectionHeading
        id="startup-offlimits-heading"
        title={t("startup.offLimits.title")}
        description={t("startup.offLimits.description")}
        aside={
          <span className="eyebrow">
            <DataValue partial={partial}>
              {tCount(items.length, "unit.startupItem.one", "unit.startupItem.other")}
            </DataValue>
          </span>
        }
      />

      {items.length === 0 ? (
        <p className="microcopy">{t("startup.offLimits.empty")}</p>
      ) : (
        <ul className="offlimits-list">
          {items.map((item) => (
            <li key={item.id}>
              <div className="offlimits-list__title">
                <StatusPill tone="attention">{t("startup.tier.offLimits")}</StatusPill>
                <h3>{item.name}</h3>
              </div>
              <p>{t("startup.item.source", { source: item.sourceLabel })}</p>
              <p className="offlimits-list__reason">
                <strong>{t("startup.item.offLimitsWhy")}</strong> {item.why}
              </p>
              <p className="microcopy">{t("startup.item.noAction")}</p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function StartupContent({
  items,
  totalEnabled,
  truncated,
  coverage
}: {
  items: StartupItem[];
  totalEnabled: number | null;
  truncated: number | null;
  coverage: DataCoverage;
}) {
  const partial = coverage.completeness !== "complete";
  const offLimits = items.filter((item) => item.tier === "OFF_LIMITS");
  const actionable = items.filter((item) => item.tier !== "OFF_LIMITS");
  const knownEmpty =
    items.length === 0 &&
    (totalEnabled === null || totalEnabled === 0) &&
    (truncated === null || truncated === 0);

  if (knownEmpty) {
    return (
      <>
        <CoverageBanner
          complete={!partial}
          coverage={coverage}
          context="scan"
        />
        <EmptyState
          title={t("startup.empty.title")}
          body={t("startup.empty.body")}
          next={t("startup.empty.next")}
        />
      </>
    );
  }

  return (
    <>
      <CoverageBanner complete={!partial} coverage={coverage} context="scan" />

      <dl className="summary-facts">
        <div>
          <dt>{t("startup.summary.totalEnabled")}</dt>
          <dd>
            <DataValue partial={partial}>{startupCount(totalEnabled)}</DataValue>
          </dd>
        </div>
        <div>
          <dt>{t("startup.summary.items")}</dt>
          <dd>
            <DataValue partial={partial}>{startupCount(items.length)}</DataValue>
          </dd>
        </div>
        <div>
          <dt>{t("startup.summary.truncated")}</dt>
          <dd>
            <DataValue partial={partial}>{startupCount(truncated)}</DataValue>
          </dd>
        </div>
      </dl>

      {truncated !== null && truncated > 0 ? (
        <InlineNotice tone="attention">
          <strong>{t("startup.truncated.title")}</strong>
          <p>
            {tCount(
              truncated,
              "startup.truncated.one",
              "startup.truncated.other"
            )}
          </p>
        </InlineNotice>
      ) : null}

      <h3>{t("startup.actionable.title")}</h3>
      {actionable.length === 0 ? (
        <p className="microcopy">{t("startup.actionable.empty")}</p>
      ) : (
        <ul className="cleanup-list">
          {actionable.map((item) => (
            <StartupItemCard item={item} key={item.id} />
          ))}
        </ul>
      )}

      <StartupOffLimits items={offLimits} partial={partial} />
    </>
  );
}

export function StartupPanel({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getStartupReport(), [api]);

  return (
    <PanelFrame
      id="startup-panel-heading"
      title={t("startup.title")}
      description={t("startup.description")}
    >
      <InlineNotice tone="attention">
        <strong>{t("startup.notice.title")}</strong>
        <p>{t("startup.notice.body")}</p>
      </InlineNotice>

      {state.status === "loading" ? <PanelLoading label={t("startup.loading")} /> : null}
      {state.status === "error" ? <PanelFailure error={state.error} /> : null}
      {state.status === "ready" ? (
        <StartupContent
          items={state.data.items}
          totalEnabled={state.data.totalEnabled}
          truncated={state.data.truncated}
          coverage={state.data.coverage}
        />
      ) : null}
    </PanelFrame>
  );
}
