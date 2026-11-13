import type {
  CategoryTotals,
  CoreSightApi,
  Drive,
  HomeSummary,
  SafetyTier,
  StorageData
} from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatBytes,
  formatDateTime,
  formatDelta,
  formatPercent
} from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";
import {
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  ScreenError,
  ScreenLoading,
  ShareBar,
  StatusPill,
  UnknownValue,
  type PillTone
} from "../components/ui";
import { Treemap, type TreemapNode } from "../components/viz";
import { PrivacyPanel } from "./PrivacyPanel";
import { PanelUnavailable, isUnavailableError } from "./PanelFrame";

function driveStatus(drive: Drive): { label: MessageKey; tone: PillTone } {
  if (drive.connection === "remembered") {
    return { label: "drive.connection.remembered", tone: "quiet" };
  }
  if (drive.health === "attention") {
    return { label: "drive.health.attention", tone: "attention" };
  }
  if (drive.health === "healthy") {
    return { label: "drive.health.healthy", tone: "positive" };
  }
  return { label: "drive.health.unknown", tone: "quiet" };
}

function opportunityCount(value: number | null): string {
  if (value === null) return t("home.hero.countUnknown");
  return tCount(value, "unit.opportunityToReview.one", "unit.opportunityToReview.other");
}

function observedEntryCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.observedEntry.one", "unit.observedEntry.other");
}

function categoryIsPartial(category: CategoryTotals, overallPartial: boolean): boolean {
  return (
    overallPartial ||
    category.entriesWithoutSize === null ||
    category.entriesWithoutSize > 0 ||
    category.observationErrors === null ||
    category.observationErrors > 0
  );
}

/**
 * Build the treemap nodes.
 *
 * The tier comes from the cleanup plan, so a category the app has classified
 * as reclaimable is tinted as such and an off-limits one is not. A category
 * the cleanup plan says nothing about stays neutral — the map must not imply
 * a safety judgement it does not have.
 */
function mapItemsFrom(
  categories: CategoryTotals[],
  tiers: Map<string, SafetyTier>,
  unmeasuredShare: number
): TreemapNode[] {
  const items: TreemapNode[] = categories.map((category) => {
    const tier = tiers.get(category.id);
    return {
      id: category.id,
      label: category.name,
      bytes: category.bytes,
      ...(tier
        ? {
            tier:
              tier === "SAFE"
                ? ("SAFE" as const)
                : tier === "REVIEW"
                  ? ("REVIEW" as const)
                  : ("OFF_LIMITS" as const)
          }
        : {})
    };
  });
  // The part of the disk the scan could not measure is not zero. It is passed
  // through as an unknown node so the map draws it as its own hatched band
  // instead of quietly claiming to account for the whole disk.
  if (unmeasuredShare > 0.005) {
    items.push({
      id: "unmeasured",
      label: t("state.unknown"),
      bytes: null
    });
  }
  return items;
}

/**
 * The legend dot shows the SAFETY TIER, not an arbitrary index.
 *
 * Two categories in the same tier share a colour, and a category the cleanup
 * plan says nothing about stays neutral — the legend must not imply a safety
 * judgement the app does not have.
 */
function tierDotClass(tier: SafetyTier | undefined): string {
  if (tier === "SAFE") return "cs-map-dot--safe";
  if (tier === "REVIEW") return "cs-map-dot--review";
  if (tier === "OFF_LIMITS") return "cs-map-dot--offlimits";
  return "cs-map-dot--neutral";
}

function HomeContent({
  home,
  api,
  storage
}: {
  home: HomeSummary;
  api: CoreSightApi;
  storage: StorageData | null;
}) {
  const coverage = home.coverage;
  const partial = coverage.completeness !== "complete";
  const safeActionable = home.safeReclaimableBytes;

  /*
   * The safety tier of each category comes from the cleanup plan, so the map
   * can tint what is reclaimable and what is not. A category the plan says
   * nothing about stays neutral rather than being guessed at.
   *
   * The read happens before every early return: a hook must never be called
   * conditionally, and this screen has an empty-state branch.
   */
  const cleanup = useApiData(() => api.getCleanup(), [api]);
  const tiers = new Map<string, SafetyTier>();
  if (cleanup.status === "ready") {
    for (const item of cleanup.data.items) {
      tiers.set(item.categoryId, item.tier);
    }
  }

  const primaryLabel =
    partial || safeActionable === null || home.actionableOpportunityCount === 0
      ? t("home.cta.openCleanup")
      : t("home.cta.reviewSafe", { bytes: formatBytes(safeActionable) });

  if (home.drives.length === 0) {
    return (
      <div className="screen screen--home">
        <PageHeader
          eyebrow={t("home.eyebrow")}
          title={t("home.title")}
          description={t("home.empty.description")}
        />
        <CoverageBanner
          complete={!partial}
          coverage={coverage}
          context="storageScan"
        />
        <EmptyState
          title={t("home.empty.title")}
          body={t("home.empty.body")}
          next={t("home.empty.next")}
        />
        <div className="home-privacy">
          <PrivacyPanel api={api} />
        </div>
      </div>
    );
  }

  const categories = [...(storage?.categories ?? home.topCategories)].sort(
    (left, right) => (right.bytes ?? 0) - (left.bytes ?? 0)
  );
  const unmeasuredShare = Math.max(0, 1 - categories.reduce(
    (sum, category) => sum + (category.shareOfParent ?? 0),
    0
  ));
  const mapItems = mapItemsFrom(categories, tiers, unmeasuredShare);
  const totalBytes = storage?.totalObservedBytes ?? null;
  const totalItems = storage?.totalObservedItems ?? null;

  const recentChange = home.changeSinceLastScan;
  const growth = recentChange?.categoryDeltas
    .filter((delta) => delta.bytesDelta !== null && delta.bytesDelta > 0)
    .slice(0, 3);
  const maxGrowth = Math.max(...(growth ?? []).map((delta) => delta.bytesDelta ?? 0), 1);

  return (
    <div className="screen screen--home">
      <PageHeader
        eyebrow={t("home.eyebrow")}
        title={t("home.title")}
        description={t("home.description")}
        aside={
          <div className="snapshot-stamp">
            <span>{t("home.stamp.lastScan")}</span>
            <strong>{formatDateTime(home.lastScanAt)}</strong>
            <span className="snapshot-stamp__note">
              {partial ? t("home.stamp.partial") : t("home.stamp.localOnly")}
            </span>
          </div>
        }
      />

      <CoverageBanner complete={!partial} coverage={coverage} context="storageScan" />

      <div className="home-layout">
        <section className="home-stage" aria-labelledby="largest-categories">
          <header className="home-stage__head">
            <div>
              <h2 id="largest-categories">{t("home.categories.title")}</h2>
              <p>{t("home.categories.description")}</p>
            </div>
            <a className="cs-link" href="#storage">
              {t("home.categories.explore")}
            </a>
          </header>

          <Treemap
            nodes={mapItems}
            height={300}
          />

          <div className="home-stage__foot">
            <span className="home-stage__total cs-num">
              {totalBytes === null ? <UnknownValue /> : formatBytes(totalBytes)}
            </span>
            <span className="home-stage__foot-label">
              {t("storage.summary.observedTotal")}
            </span>
            <StatusPill tone={partial ? "attention" : "positive"}>
              {t(partial ? "status.partial" : "status.complete")}
            </StatusPill>
            {totalItems !== null ? (
              <span className="home-stage__basis">
                {t(
                  partial ? "storage.summary.bodyPartial" : "storage.summary.bodyComplete",
                  { count: observedEntryCount(totalItems) }
                )}
              </span>
            ) : null}
          </div>

          {/*
            The explanation of the map sits directly beneath the map, not at
            the foot of the page. A proportional view that does not say what
            its areas mean is decoration; one that does is comprehension.
          */}
          <section className="home-caveat" aria-label={t("home.caveat.aria")}>
            <p>
              <strong>{t("home.caveat.title")}</strong> {t("home.caveat.body")}
            </p>
          </section>

          <div className="home-legend__head" aria-hidden="true">
            <span>{t("storage.column.category")}</span>
            <span>{t("storage.column.shareOfTotal")}</span>
            <span>{t("storage.column.observed")}</span>
            <span>{t("storage.column.change")}</span>
          </div>
          <ol className="home-legend" aria-labelledby="largest-categories">
            {categories.map((category) => {
              const categoryPartial = categoryIsPartial(category, partial);
              return (
                <li key={category.id}>
                  <a className="home-legend__row" href="#storage">
                    <span className="home-legend__name">
                      <span
                        className={`cs-map-dot ${tierDotClass(tiers.get(category.id))}`}
                        aria-hidden="true"
                      />
                      <strong>{category.name}</strong>
                    </span>
                    <span className="home-legend__share">
                      <ShareBar
                        share={category.shareOfParent}
                        label={t("home.category.shareLabel", { name: category.name })}
                        partial={categoryPartial}
                      />
                      <span className="cs-num">
                        {category.shareOfParent === null ? (
                          <UnknownValue />
                        ) : (
                          formatPercent(category.shareOfParent)
                        )}
                      </span>
                    </span>
                    <span className="home-legend__bytes cs-num">
                      <Bytes value={category.bytes} partial={categoryPartial} />
                    </span>
                    <span className="home-legend__delta cs-num">
                      {formatDelta(category.deltaSinceLast)}
                    </span>
                  </a>
                </li>
              );
            })}
            {unmeasuredShare > 0.005 ? (
              <li>
                <div className="home-legend__row home-legend__row--unknown">
                  <span className="home-legend__name">
                    <span className="cs-map-dot cs-map-dot--unknown" aria-hidden="true" />
                    <UnknownValue />
                  </span>
                  <span className="home-legend__share">
                    <span className="cs-num">
                      {formatPercent(unmeasuredShare)}
                    </span>
                  </span>
                  <span className="home-legend__bytes">
                    <UnknownValue />
                  </span>
                  <span className="home-legend__delta">{t("format.shareUnknown")}</span>
                </div>
              </li>
            ) : null}
          </ol>
        </section>

        <aside className="home-rail">
          <section className="home-reclaim cs-surface cs-surface--raised cs-surface--padded">
            <h2 className="home-reclaim__label">{t("home.safeToReclaim")}</h2>
            <p className="home-reclaim__figure cs-num">
              {safeActionable === null ? (
                <UnknownValue />
              ) : (
                <DataValue partial={partial}>{formatBytes(safeActionable)}</DataValue>
              )}
            </p>
            <p className="home-reclaim__context">
              <DataValue partial={partial}>
                {opportunityCount(home.actionableOpportunityCount)}
              </DataValue>
              <span aria-hidden="true"> · </span>
              <DataValue partial={partial}>
                {home.reviewReclaimableBytes === null ? (
                  t("home.reviewAmountUnknown")
                ) : (
                  t("home.reviewInOpportunities", {
                    bytes: formatBytes(home.reviewReclaimableBytes)
                  })
                )}
              </DataValue>
            </p>
            <p className="home-reclaim__basis">
              {t(partial ? "home.basisPartial" : "home.basisComplete", {
                time: formatDateTime(home.lastScanAt)
              })}
            </p>
            <p className="home-reclaim__rule">{t("home.rule")}</p>
            <a className="cs-button cs-button--primary" href="#cleanup">
              {primaryLabel}
            </a>
            {partial ? (
              <p className="home-reclaim__blocked">{t("home.cleanupBlocked")}</p>
            ) : null}
          </section>

          <section className="home-drives" aria-labelledby="home-drives-heading">
            <div className="home-rail__head">
              <h2 id="home-drives-heading">{t("home.driveHealth")}</h2>
              <span className="cs-eyebrow">{t("drives.overview.connected")}</span>
            </div>
            {home.drives.map((drive) => {
              const status = driveStatus(drive);
              return (
                <article className="drive-compact" key={drive.id}>
                  <div className="drive-compact__top">
                    <div>
                      <h3>{drive.name}</h3>
                      <p>
                        {t(
                          drive.kind === "external"
                            ? "drive.kind.external"
                            : "drive.kind.startupVolume"
                        )}
                        {drive.connection === "remembered"
                          ? t("home.drive.lastKnownSuffix")
                          : ""}
                      </p>
                    </div>
                    <StatusPill tone={status.tone}>{t(status.label)}</StatusPill>
                  </div>
                  <ShareBar
                    share={drive.usedShare}
                    label={t("home.drive.usedSpaceLabel", { name: drive.name })}
                    partial={partial}
                  />
                  <div className="drive-compact__numbers">
                    <strong>
                      {drive.freeBytes === null ? (
                        <UnknownValue label={t("home.drive.freeUnknownValue")} />
                      ) : (
                        t("home.drive.free", { bytes: formatBytes(drive.freeBytes) })
                      )}
                    </strong>
                    <span>
                      {drive.usedBytes === null || drive.totalBytes === null ? (
                        <UnknownValue />
                      ) : (
                        t("home.drive.usedOfTotal", {
                          used: formatBytes(drive.usedBytes),
                          total: formatBytes(drive.totalBytes)
                        })
                      )}
                    </span>
                  </div>
                  {drive.freeBytes === null ? (
                    <p className="cs-micro">{t("home.drive.freeUnknown")}</p>
                  ) : null}
                </article>
              );
            })}
          </section>

          <section className="home-change" aria-labelledby="since-last-scan">
            <div className="home-rail__head">
              <h2 id="since-last-scan">{t("home.since.title")}</h2>
              <a className="cs-link" href="#history">
                {t("home.change.viewHistory")}
              </a>
            </div>
            {recentChange === null || recentChange.totalBytesDelta === null ? (
              <div className="cs-unknown-block">
                <strong>{t("home.change.unknown")}</strong>
                <p>{t("home.change.unknownBody")}</p>
                <a className="cs-link" href="#history">
                  {t("home.change.viewHistory")}
                </a>
              </div>
            ) : (
              <>
                <p className="home-change__total">
                  <DataValue partial={recentChange.coverage.completeness !== "complete"}>
                    {t("home.change.across", {
                      delta: formatDelta(recentChange.totalBytesDelta)
                    })}
                  </DataValue>
                </p>
                <ul className="growth-list">
                  {(growth ?? []).map((delta) => (
                    <li key={delta.categoryId}>
                      <div className="growth-list__row">
                        <span>{delta.name}</span>
                        <DataValue
                          partial={
                            delta.partial ||
                            recentChange.coverage.completeness !== "complete"
                          }
                        >
                          {formatDelta(delta.bytesDelta)}
                        </DataValue>
                      </div>
                      <span className="growth-list__bar" aria-hidden="true">
                        <span
                          style={{
                            width: `${Math.max(
                              6,
                              (Math.abs(delta.bytesDelta ?? 0) / maxGrowth) * 100
                            )}%`
                          }}
                        />
                      </span>
                    </li>
                  ))}
                </ul>
                <a className="cs-link" href="#history">
                  {t("home.change.seeWhatChanged")}
                </a>
              </>
            )}
          </section>
        </aside>
      </div>

      <div className="home-privacy">
        <PrivacyPanel api={api} />
      </div>
    </div>
  );
}

export function HomeScreen({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getHomeSummary(), [api]);
  const storageState = useApiData(() => api.getStorage(), [api]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow={t("home.eyebrow")}
        title={t("home.title")}
        description={t("home.description")}
        layout="home"
      />
    );
  }
  if (state.status === "error") {
    if (isUnavailableError(state.error)) {
      return (
        <div className="screen">
          <PageHeader
            eyebrow={t("home.eyebrow")}
            title={t("home.title")}
            description={t("home.description")}
          />
          <PanelUnavailable error={state.error} />
          <div className="home-privacy">
            <PrivacyPanel api={api} />
          </div>
        </div>
      );
    }
    return (
      <ScreenError
        eyebrow={t("home.eyebrow")}
        title={t("home.title")}
        description={t("home.description")}
        error={state.error}
      />
    );
  }

  return (
    <HomeContent
      home={state.data}
      api={api}
      storage={storageState.status === "ready" ? storageState.data : null}
    />
  );
}
