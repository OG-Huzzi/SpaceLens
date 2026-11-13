import type { CoreSightApi, Drive, HomeSummary } from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatBytes,
  formatDateTime,
  formatDelta,
  formatPercent
} from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  SizeBar,
  StatusPill,
  type PillTone
} from "../components/ui";

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

function HomeContent({ home }: { home: HomeSummary }) {
  const coverage = home.coverage;
  const partial = coverage.completeness !== "complete";
  const safeActionable = home.safeReclaimableBytes;
  const primaryLabel =
    partial || safeActionable === null || home.actionableOpportunityCount === 0
      ? t("home.cta.openCleanup")
      : t("home.cta.reviewSafe", { bytes: formatBytes(safeActionable) });

  if (home.drives.length === 0) {
    return (
      <div className="screen">
        <PageHeader
          eyebrow={t("home.eyebrow")}
          title={t("home.title")}
          description={t("home.empty.description")}
        />
        <CoverageBanner coverage={coverage} context="storageScan" />
        <EmptyState
          title={t("home.empty.title")}
          description={t("home.empty.body")}
          nextStep={t("home.empty.next")}
        />
      </div>
    );
  }

  const recentChange = home.changeSinceLastScan;
  const growth = recentChange?.categoryDeltas
    .filter((delta) => delta.bytesDelta !== null && delta.bytesDelta > 0)
    .slice(0, 3);

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

      <CoverageBanner coverage={coverage} context="storageScan" />

      <section className="home-hero" aria-labelledby="reclaim-heading">
        <div className="home-hero__lead">
          <p className="eyebrow">{t("home.safeToReclaim")}</p>
          <h2 id="reclaim-heading" className="home-hero__figure">
            <DataValue partial={partial}>{formatBytes(home.safeReclaimableBytes)}</DataValue>
          </h2>
          <p className="home-hero__context">
            <DataValue partial={partial}>
              {opportunityCount(home.actionableOpportunityCount)}
            </DataValue>
            {" · "}
            <DataValue partial={partial}>
              {home.reviewReclaimableBytes === null
                ? t("home.reviewAmountUnknown")
                : t("home.reviewInOpportunities", {
                    bytes: formatBytes(home.reviewReclaimableBytes)
                  })}
            </DataValue>
            {" · "}
            {t(partial ? "home.basisPartial" : "home.basisComplete", {
              time: formatDateTime(home.lastScanAt)
            })}
          </p>
          <p className="home-hero__rule">{t("home.rule")}</p>
          <a className="button button--primary" href="#cleanup">
            {primaryLabel}
          </a>
          {partial ? <p className="home-hero__blocked">{t("home.cleanupBlocked")}</p> : null}
        </div>

        <div className="home-hero__drives">
          <span className="home-hero__vertical-label">{t("home.driveHealth")}</span>
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
                <SizeBar
                  share={drive.usedShare}
                  label={t("home.drive.usedSpaceLabel", { name: drive.name })}
                  partial={partial}
                />
                <div className="drive-compact__numbers">
                  <DataValue partial={partial}>
                    <strong>
                      {drive.freeBytes === null
                        ? t("home.drive.freeUnknownValue")
                        : t("home.drive.free", { bytes: formatBytes(drive.freeBytes) })}
                    </strong>
                  </DataValue>
                  <span>
                    {t("home.drive.usedOfTotal", {
                      used: formatBytes(drive.usedBytes),
                      total: formatBytes(drive.totalBytes)
                    })}
                  </span>
                </div>
                {drive.freeBytes === null ? (
                  <p className="microcopy">{t("home.drive.freeUnknown")}</p>
                ) : null}
              </article>
            );
          })}
        </div>
      </section>

      <div className="home-grid">
        <section className="panel" aria-labelledby="largest-categories">
          <SectionHeading
            id="largest-categories"
            title={t("home.categories.title")}
            description={t("home.categories.description")}
            aside={
              <a className="text-link" href="#storage">
                {t("home.categories.explore")}
              </a>
            }
          />
          <ol className="rank-list" aria-labelledby="largest-categories">
            {home.topCategories.map((category, index) => {
              const categoryPartial =
                partial ||
                category.entriesWithoutSize === null ||
                category.entriesWithoutSize > 0 ||
                category.observationErrors === null ||
                category.observationErrors > 0;
              return (
                <li className="rank-row" key={category.id}>
                  <span className="rank-row__index" aria-hidden="true">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                  <div className="rank-row__main">
                    <div className="rank-row__label">
                      <strong>{category.name}</strong>
                      <span>
                        <DataValue partial={categoryPartial}>
                          {t("home.category.ofUsedSpace", {
                            percent: formatPercent(category.shareOfParent)
                          })}
                        </DataValue>
                      </span>
                    </div>
                    <SizeBar
                      share={category.shareOfParent}
                      label={t("home.category.shareLabel", { name: category.name })}
                      partial={categoryPartial}
                    />
                  </div>
                  <div className="rank-row__value">
                    <DataValue partial={categoryPartial}>{formatBytes(category.bytes)}</DataValue>
                    <DataValue
                      partial={categoryPartial}
                      className={
                        category.deltaSinceLast !== null && category.deltaSinceLast > 0
                          ? "delta delta--up"
                          : "delta"
                      }
                    >
                      {formatDelta(category.deltaSinceLast)}
                    </DataValue>
                  </div>
                </li>
              );
            })}
          </ol>
        </section>

        <section className="panel panel--soft" aria-labelledby="since-last-scan">
          <SectionHeading
            id="since-last-scan"
            title={t("home.since.title")}
            description={recentChange?.note ?? t("home.since.description")}
          />
          {recentChange === null || recentChange.totalBytesDelta === null ? (
            <div className="unknown-block">
              <strong>{t("home.change.unknown")}</strong>
              <p>{t("home.change.unknownBody")}</p>
              <a className="text-link" href="#history">
                {t("home.change.viewHistory")}
              </a>
            </div>
          ) : (
            <>
              <p className="change-total">
                <DataValue partial={recentChange.coverage.completeness !== "complete"}>
                  {t("home.change.across", {
                    delta: formatDelta(recentChange.totalBytesDelta)
                  })}
                </DataValue>
              </p>
              <ul className="growth-list">
                {(growth ?? []).map((delta) => (
                  <li key={delta.categoryId}>
                    <span>{delta.name}</span>
                    <DataValue
                      partial={delta.partial || recentChange.coverage.completeness !== "complete"}
                    >
                      {formatDelta(delta.bytesDelta)}
                    </DataValue>
                  </li>
                ))}
              </ul>
              <a className="text-link" href="#history">
                {t("home.change.seeWhatChanged")}
              </a>
            </>
          )}
        </section>
      </div>

      <section className="home-footer-note" aria-label={t("home.caveat.aria")}>
        <p>
          <strong>{t("home.caveat.title")}</strong> {t("home.caveat.body")}
        </p>
      </section>
    </div>
  );
}

export function HomeScreen({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getHomeSummary(), [api]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow={t("home.eyebrow")}
        title={t("home.title")}
        description={t("home.description")}
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow={t("home.eyebrow")}
        title={t("home.title")}
        description={t("home.description")}
        error={state.error}
      />
    );
  }

  return <HomeContent home={state.data} />;
}
