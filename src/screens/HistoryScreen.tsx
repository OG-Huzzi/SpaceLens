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
  formatDateTime,
  formatDelta,
  formatSignedCount
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
  StatusPill
} from "../components/ui";
import { BlockListPanel } from "./BlockListPanel";
import { DnsRankingPanel } from "./DnsRankingPanel";
import {
  PanelUnavailable,
  isUnavailableError
} from "./PanelFrame";
import { StartupPanel } from "./StartupPanel";

const TRIGGER_LABELS: Record<HistorySnapshot["trigger"], MessageKey> = {
  "first-run": "history.trigger.firstRun",
  scheduled: "history.trigger.scheduled",
  manual: "history.trigger.manual"
};

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
            t("history.delta.entryUnknown")
          ) : delta.entriesDelta === 0 ? (
            t("history.delta.noEntryChange")
          ) : (
            <DataValue partial={partial}>
              {t(
                Math.abs(delta.entriesDelta) === 1
                  ? "history.delta.entryChange.one"
                  : "history.delta.entryChange.other",
                { count: formatSignedCount(delta.entriesDelta) }
              )}
            </DataValue>
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
        aria-label={t("history.delta.aria", {
          name: delta.name,
          delta: formatDelta(delta.bytesDelta)
        })}
      >
        {content}
        <span className="delta-row__jump" aria-hidden="true">
          {t("history.delta.open")}
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
            <p className="eyebrow">{t(TRIGGER_LABELS[snapshot.trigger])}</p>
            <h3>{formatDateTime(snapshot.takenAt)}</h3>
          </div>
          <StatusPill tone={partial ? "attention" : "positive"}>
            {t(partial ? "status.partial" : "status.complete")}
          </StatusPill>
        </header>
        <dl className="timeline-item__facts">
          <div>
            <dt>{t("history.snapshot.observedTotal")}</dt>
            <dd>
              <DataValue partial={partial}>{formatBytes(snapshot.totalBytes)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>{t("history.snapshot.changeVsPrevious")}</dt>
            <dd>
              {snapshot.deltaBytes === null ? (
                t("history.snapshot.noEarlier")
              ) : (
                <DataValue partial={partial}>{formatDelta(snapshot.deltaBytes)}</DataValue>
              )}
            </dd>
          </div>
          <div>
            <dt>{t("history.snapshot.entries")}</dt>
            <dd>
              <DataValue partial={partial}>
                {snapshot.entries === null
                  ? t("state.unknown")
                  : tCount(snapshot.entries, "unit.entry.one", "unit.entry.other")}
              </DataValue>
            </dd>
          </div>
        </dl>
      </article>
    </li>
  );
}

function SystemRecords({ api }: { api: CoreSightApi }) {
  return (
    <section aria-labelledby="history-records-heading" style={{ marginTop: "32px" }}>
      <SectionHeading
        id="history-records-heading"
        title={t("history.records.title")}
        description={t("history.records.description")}
      />
      <div style={{ display: "grid", gap: "20px" }}>
        <StartupPanel api={api} />
        <DnsRankingPanel api={api} />
        <BlockListPanel api={api} />
      </div>
    </section>
  );
}

function HistoryContent({
  api,
  data,
  onOpenCategory
}: {
  api: CoreSightApi;
  data: HistoryData;
  onOpenCategory: (categoryId: string) => void;
}) {
  const partial = data.coverage.completeness !== "complete";
  const change = data.changeSinceLastScan;
  const canCompare = change !== null && data.snapshots.length >= 2;

  return (
    <div className="screen screen--history">
      <PageHeader
        eyebrow={t("history.eyebrow")}
        title={t("history.title")}
        description={t("history.description")}
        aside={
          <div className="snapshot-stamp">
            <span>{t("history.stamp.title")}</span>
            <strong>
              <DataValue partial={partial}>
                {tCount(data.snapshots.length, "unit.snapshot.one", "unit.snapshot.other")}
              </DataValue>
            </strong>
            <span className="snapshot-stamp__note">{t("history.stamp.note")}</span>
          </div>
        }
      />

      <CoverageBanner coverage={data.coverage} />

      {data.snapshots.length === 0 ? (
        <EmptyState
          title={t("history.empty.title")}
          description={t("history.empty.body")}
          nextStep={t("history.empty.next")}
        />
      ) : (
        <>
          <section className="panel history-change" aria-labelledby="change-heading">
            <SectionHeading
              id="change-heading"
              title={t("history.change.title")}
              description={change?.note ?? t("history.change.descriptionFallback")}
              aside={
                canCompare ? (
                  <StatusPill tone={partial ? "attention" : "positive"}>
                    {t(partial ? "status.partialComparison" : "status.comparableCoverage")}
                  </StatusPill>
                ) : (
                  <StatusPill tone="quiet">{t("status.baselineOnly")}</StatusPill>
                )
              }
            />
            {canCompare && change ? (
              <>
                <div className="change-hero">
                  <span>{t("history.change.total")}</span>
                  <strong>
                    <DataValue partial={partial}>{formatDelta(change.totalBytesDelta)}</DataValue>
                  </strong>
                  <p>
                    {t("history.change.range", {
                      from: formatDateTime(change.fromAt),
                      to: formatDateTime(change.toAt),
                      coverage: t(
                        change.coverage.completeness === "complete"
                          ? "history.change.completeCoverage"
                          : "history.change.partialCoverage"
                      )
                    })}
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
                <strong>{t("history.change.unknown")}</strong>
                <p>{t("history.change.unknownBody")}</p>
              </div>
            )}
          </section>

          <section className="panel history-timeline" aria-labelledby="timeline-heading">
            <SectionHeading
              id="timeline-heading"
              title={t("history.timeline.title")}
              description={t("history.timeline.description")}
            />
            <ol className="timeline-list">
              {data.snapshots.map((snapshot, index) => (
                <SnapshotRow snapshot={snapshot} index={index} key={snapshot.id} />
              ))}
            </ol>
          </section>
        </>
      )}

      {/*
        These records are deliberately read-only. History is the screen that
        remembers what the system held; it must not become a place where a
        startup item can be disabled, DNS can be switched, or a block list can
        be applied. Each panel owns its own loading/error/unavailable state so a
        missing engine inventory never degrades into a zero or empty success.
      */}
      <SystemRecords api={api} />
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
        eyebrow={t("history.eyebrow")}
        title={t("history.title")}
        description={t("history.description")}
      />
    );
  }
  if (state.status === "error") {
    if (isUnavailableError(state.error)) {
      return (
        <div className="screen">
          <PageHeader
            eyebrow={t("history.eyebrow")}
            title={t("history.title")}
            description={t("history.description")}
          />
          <PanelUnavailable error={state.error} />
          <SystemRecords api={api} />
        </div>
      );
    }
    return (
      <ScreenError
        eyebrow={t("history.eyebrow")}
        title={t("history.title")}
        description={t("history.description")}
        error={state.error}
      />
    );
  }

  return (
    <HistoryContent api={api} data={state.data} onOpenCategory={onOpenCategory} />
  );
}
