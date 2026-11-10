import type {
  CategoryDelta,
  ChangeSinceLastScan,
  CoreSightApi,
  HistoryData,
  HistorySnapshot
} from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatBytes,
  formatCount,
  formatDateTime,
  formatDelta,
  formatSignedCount
} from "../lib/format";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  StatusPill
} from "../components/ui";

function isPartialDelta(delta: CategoryDelta, change: ChangeSinceLastScan): boolean {
  return delta.partial || change.coverage.completeness !== "complete";
}

function DeltaRow({
  delta,
  change,
  onOpenCategory
}: {
  delta: CategoryDelta;
  change: ChangeSinceLastScan;
  onOpenCategory: (categoryId: string) => void;
}) {
  const partial = isPartialDelta(delta, change);
  const changed = delta.bytesDelta !== null && delta.bytesDelta !== 0;
  const content = (
    <>
      <span className="delta-row__name">
        <strong>{delta.name}</strong>
        <span>
          {delta.entriesDelta === null ? (
            "Entry change unknown"
          ) : delta.entriesDelta === 0 ? (
            "No entry change"
          ) : (
            <>
              <DataValue partial={partial}>
                {formatSignedCount(delta.entriesDelta)}
              </DataValue>{" "}
              entries
            </>
          )}
        </span>
      </span>
      <span
        className={`delta-row__amount${
          delta.bytesDelta !== null && delta.bytesDelta > 0 ? " delta delta--up" : " delta"
        }`}
      >
        <DataValue partial={partial}>{formatDelta(delta.bytesDelta)}</DataValue>
      </span>
      <span className="delta-row__sides">
        {formatBytes(delta.oldBytes)} <span aria-hidden="true">→</span>{" "}
        {formatBytes(delta.newBytes)}
      </span>
    </>
  );

  if (!changed) {
    return <li className="delta-row delta-row--static">{content}</li>;
  }

  return (
    <li>
      <a
        className="delta-row"
        href="#storage"
        onClick={() => onOpenCategory(delta.categoryId)}
        aria-label={`${delta.name}: ${formatDelta(delta.bytesDelta)} since the previous snapshot. Open this category in Storage.`}
      >
        {content}
        <span className="delta-row__jump" aria-hidden="true">
          Open in Storage ↗
        </span>
      </a>
    </li>
  );
}

function SnapshotRow({ snapshot, index }: { snapshot: HistorySnapshot; index: number }) {
  const partial = snapshot.coverage.completeness !== "complete";
  return (
    <li className="timeline-item">
      <span className="timeline-item__marker" aria-hidden="true">
        {String(index + 1).padStart(2, "0")}
      </span>
      <article className="timeline-item__body">
        <header>
          <div>
            <p className="eyebrow">
              {snapshot.trigger === "first-run"
                ? "First baseline"
                : snapshot.trigger === "scheduled"
                  ? "Scheduled snapshot"
                  : "Manual snapshot"}
            </p>
            <h3>{formatDateTime(snapshot.takenAt)}</h3>
          </div>
          <StatusPill tone={partial ? "attention" : "positive"}>
            {snapshot.coverage.completeness === "complete" ? "Complete" : "Partial"}
          </StatusPill>
        </header>
        <dl className="timeline-item__facts">
          <div>
            <dt>Observed total</dt>
            <dd>
              <DataValue partial={partial}>{formatBytes(snapshot.totalBytes)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>Change vs previous</dt>
            <dd>
              {snapshot.deltaBytes === null ? (
                "No earlier snapshot"
              ) : (
                <DataValue partial={partial}>{formatDelta(snapshot.deltaBytes)}</DataValue>
              )}
            </dd>
          </div>
          <div>
            <dt>Entries</dt>
            <dd>
              <DataValue partial={partial}>
                {formatCount(snapshot.entries, "entry", "entries")}
              </DataValue>
            </dd>
          </div>
        </dl>
      </article>
    </li>
  );
}

function HistoryContent({
  data,
  onOpenCategory
}: {
  data: HistoryData;
  onOpenCategory: (categoryId: string) => void;
}) {
  const partial = data.coverage.completeness !== "complete";
  const change = data.changeSinceLastScan;
  const canCompare = change !== null && data.snapshots.length >= 2;

  return (
    <div className="screen screen--history">
      <PageHeader
        eyebrow="History"
        title="What changed?"
        description="Snapshots answer the +80 GB question with dated, comparable evidence — never with a guessed trend."
        aside={
          <div className="snapshot-stamp">
            <span>Snapshots stored locally</span>
            <strong>
              <DataValue partial={partial}>
                {formatCount(data.snapshots.length, "snapshot", "snapshots")}
              </DataValue>
            </strong>
            <span className="snapshot-stamp__note">Local database · nothing uploaded</span>
          </div>
        }
      />

      <CoverageBanner coverage={data.coverage} />

      {data.snapshots.length === 0 ? (
        <EmptyState
          title="No snapshots yet"
          description="There is no history to compare. CoreSight does not invent a baseline or show a zero change for missing data."
          nextStep="Scan twice to see change. The first completed scan becomes the baseline; the next one produces the first comparison."
        />
      ) : (
        <>
          <section className="panel history-change" aria-labelledby="change-heading">
            <SectionHeading
              id="change-heading"
              title="Since the previous snapshot"
              description={
                change?.note ??
                "A comparison is not available yet. One valid baseline cannot answer what changed."
              }
              aside={
                canCompare ? (
                  <StatusPill tone={partial ? "attention" : "positive"}>
                    {partial ? "Partial comparison" : "Comparable coverage"}
                  </StatusPill>
                ) : (
                  <StatusPill tone="quiet">Baseline only</StatusPill>
                )
              }
            />
            {canCompare && change ? (
              <>
                <div className="change-hero">
                  <span>Total observed change</span>
                  <strong>
                    <DataValue partial={partial}>{formatDelta(change.totalBytesDelta)}</DataValue>
                  </strong>
                  <p>
                    From {formatDateTime(change.fromAt)} to {formatDateTime(change.toAt)}.{" "}
                    {change.coverage.completeness === "complete"
                      ? "Both snapshots had comparable, complete coverage."
                      : "At least one snapshot was partial; treat this as a lower-bound change."}
                  </p>
                </div>
                <ul className="delta-list">
                  {change.categoryDeltas.map((delta) => (
                    <DeltaRow
                      key={delta.categoryId}
                      delta={delta}
                      change={change}
                      onOpenCategory={onOpenCategory}
                    />
                  ))}
                </ul>
              </>
            ) : (
              <div className="unknown-block">
                <strong>Change unknown</strong>
                <p>
                  A second completed snapshot is required. The baseline below is a fact, not a
                  claim that nothing has changed since.
                </p>
              </div>
            )}
          </section>

          <section className="panel history-timeline" aria-labelledby="timeline-heading">
            <SectionHeading
              id="timeline-heading"
              title="Snapshot timeline"
              description="Newest first. Each total carries its coverage so a partial scan cannot read as complete."
            />
            <ol className="timeline-list">
              {data.snapshots.map((snapshot, index) => (
                <SnapshotRow snapshot={snapshot} index={index} key={snapshot.id} />
              ))}
            </ol>
          </section>
        </>
      )}
    </div>
  );
}

export function HistoryScreen({
  api,
  onOpenCategory
}: {
  api: CoreSightApi;
  onOpenCategory: (categoryId: string) => void;
}) {
  const state = useApiData(() => api.getHistory(), [api]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow="History"
        title="What changed?"
        description="Snapshots answer the +80 GB question with dated, comparable evidence — never with a guessed trend."
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow="History"
        title="What changed?"
        description="Snapshots answer the +80 GB question with dated, comparable evidence — never with a guessed trend."
        error={state.error}
      />
    );
  }

  return <HistoryContent data={state.data} onOpenCategory={onOpenCategory} />;
}
