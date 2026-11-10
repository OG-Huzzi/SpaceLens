import type { CoreSightApi, Drive, HomeSummary } from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatBytes,
  formatCount,
  formatDateTime,
  formatDelta,
  formatPercent
} from "../lib/format";
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

function driveStatus(drive: Drive): { label: string; tone: PillTone } {
  if (drive.connection === "remembered") return { label: "Remembered", tone: "quiet" };
  if (drive.health === "attention") return { label: "Needs attention", tone: "attention" };
  if (drive.health === "healthy") return { label: "Healthy", tone: "positive" };
  return { label: "Health unknown", tone: "quiet" };
}

function HomeContent({ home }: { home: HomeSummary }) {
  const coverage = home.coverage;
  const partial = coverage.completeness !== "complete";
  const safeActionable = home.safeReclaimableBytes;
  const primaryLabel =
    partial || safeActionable === null || home.actionableOpportunityCount === 0
      ? "Open cleanup review"
      : `Review ${formatBytes(safeActionable)} you can safely remove`;

  if (home.drives.length === 0) {
    return (
      <div className="screen">
        <PageHeader
          eyebrow="Home"
          title="How is your storage doing?"
          description="One calm summary of the drives CoreSight can see, with every number tied to its context."
        />
        <EmptyState
          title="No drive summary yet"
          description="CoreSight has not completed a storage scan on this device. Nothing is being measured in the background."
          nextStep="Once a scan completes, this page will show free space, the largest categories, and what changed."
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
        eyebrow="Home"
        title="How is your storage doing?"
        description="A calm reading of the last local snapshot. Context first, technical detail later."
        aside={
          <div className="snapshot-stamp">
            <span>Last completed scan</span>
            <strong>{formatDateTime(home.lastScanAt)}</strong>
            <span className="snapshot-stamp__note">
              {partial ? "Partial scan · totals incomplete" : "Local only · never uploaded"}
            </span>
          </div>
        }
      />

      <CoverageBanner coverage={coverage} context="storage scan" />

      <section className="home-hero" aria-labelledby="reclaim-heading">
        <div className="home-hero__lead">
          <p className="eyebrow">Safe to reclaim</p>
          <h2 id="reclaim-heading" className="home-hero__figure">
            <DataValue partial={partial}>{formatBytes(home.safeReclaimableBytes)}</DataValue>
          </h2>
          <p className="home-hero__context">
            <DataValue partial={partial}>
              {formatCount(
                home.actionableOpportunityCount,
                "opportunity to review",
                "opportunities to review"
              )}
            </DataValue>
            {" · "}
            <DataValue partial={partial}>
              {home.reviewReclaimableBytes === null
                ? "review amount unknown"
                : `${formatBytes(home.reviewReclaimableBytes)} more needs your review`}
            </DataValue>
            {" · "}
            based on the {partial ? "partial " : ""}scan completed{" "}
            {formatDateTime(home.lastScanAt)}.
          </p>
          <p className="home-hero__rule">
            Nothing moves automatically. Every cleanup is previewed and confirmed one opportunity
            at a time.
          </p>
          <a className="button button--primary" href="#cleanup">
            {primaryLabel}
          </a>
          {partial ? (
            <p className="home-hero__blocked">
              This scan is incomplete, so cleanup is blocked until a complete scan succeeds.
            </p>
          ) : null}
        </div>

        <div className="home-hero__drives">
          <span className="home-hero__vertical-label">Drive health</span>
          {home.drives.map((drive) => {
            const status = driveStatus(drive);
            return (
              <article className="drive-compact" key={drive.id}>
                <div className="drive-compact__top">
                  <div>
                    <h3>{drive.name}</h3>
                    <p>
                      {drive.kind === "external" ? "External drive" : "Startup volume"}
                      {drive.connection === "remembered" ? " · last known" : ""}
                    </p>
                  </div>
                  <StatusPill tone={status.tone}>{status.label}</StatusPill>
                </div>
                <SizeBar
                  share={drive.usedShare}
                  label={`${drive.name} used space`}
                  partial={partial}
                />
                <div className="drive-compact__numbers">
                  <DataValue partial={partial}>
                    <strong>{formatBytes(drive.freeBytes)}</strong> free
                  </DataValue>
                  <span>
                    <DataValue partial={partial}>{formatBytes(drive.usedBytes)}</DataValue> used
                    of <DataValue partial={partial}>{formatBytes(drive.totalBytes)}</DataValue>
                  </span>
                </div>
                {drive.freeBytes === null ? (
                  <p className="microcopy">
                    Free space is unknown while this drive is disconnected, not zero.
                  </p>
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
            title="Largest categories"
            description="Human categories first. Open Storage for subcategories and sample locations."
            aside={<a className="text-link" href="#storage">Explore storage</a>}
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
                          {formatPercent(category.shareOfParent)} of used space
                        </DataValue>
                      </span>
                    </div>
                    <SizeBar
                      share={category.shareOfParent}
                      label={`${category.name} share of used space`}
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
            title="Since last scan"
            description={
              recentChange?.note ??
              "The previous snapshot could not be compared, so no change is claimed."
            }
          />
          {recentChange === null || recentChange.totalBytesDelta === null ? (
            <div className="unknown-block">
              <strong>Change unknown</strong>
              <p>
                Scan twice to establish a comparison. An absent comparison is not the same as
                “nothing changed”.
              </p>
              <a className="text-link" href="#history">View scan history</a>
            </div>
          ) : (
            <>
              <p className="change-total">
                <DataValue
                  partial={recentChange.coverage.completeness !== "complete"}
                >
                  {formatDelta(recentChange.totalBytesDelta)}
                </DataValue>
                <span> across the categories you can see</span>
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
              <a className="text-link" href="#history">See what changed</a>
            </>
          )}
        </section>
      </div>

      <section className="home-footer-note" aria-label="Snapshot caveat">
        <p>
          <strong>How to read these numbers.</strong> Sizes are observed logical bytes, not
          promises of recoverable space. A category delta is labelled partial whenever either
          snapshot was incomplete. Unknown values stay unknown.
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
        eyebrow="Home"
        title="How is your storage doing?"
        description="A calm reading of the last local snapshot. Context first, technical detail later."
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow="Home"
        title="How is your storage doing?"
        description="A calm reading of the last local snapshot. Context first, technical detail later."
        error={state.error}
      />
    );
  }

  return <HomeContent home={state.data} />;
}
