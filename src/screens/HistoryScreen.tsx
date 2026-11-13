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
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  Panel,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  StatusPill,
  UnknownValue
} from "../components/ui";
import { BlockListPanel } from "./BlockListPanel";
import { DnsRankingPanel } from "./DnsRankingPanel";
import { PanelUnavailable, isUnavailableError } from "./PanelFrame";
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
  maxDelta,
  onOpenCategory
}: {
  delta: CategoryDelta;
  change: ChangeSinceLastScan;
  maxDelta: number;
  onOpenCategory: (categoryId: string) => void;
}) {
  const partial = isPartialDelta(delta, change);
  const changed = delta.bytesDelta !== null && delta.bytesDelta !== 0;
  const direction =
    delta.bytesDelta !== null && delta.bytesDelta > 0 ? "up" : "down";
  const width =
    delta.bytesDelta === null ? 0 : (Math.abs(delta.bytesDelta) / maxDelta) * 50;

  const content = (
    <>
      <span className="delta-ledger__name">
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
      <span className="delta-ledger__track" aria-hidden="true">
        {delta.bytesDelta === null ? (
          <span className="delta-ledger__unknown" />
        ) : (
          <span
            className={`delta-ledger__fill delta-ledger__fill--${direction}`}
            style={{ width: `${Math.max(width, delta.bytesDelta === 0 ? 0 : 2)}%` }}
          />
        )}
      </span>
      <span className="delta-ledger__amount cs-num">
        <DataValue partial={partial}>{formatDelta(delta.bytesDelta)}</DataValue>
      </span>
      <span className="delta-ledger__sides cs-num">
        {delta.oldBytes === null ? (
          <UnknownValue />
        ) : (
          formatBytes(delta.oldBytes)
        )}{" "}
        <span aria-hidden="true">→</span>{" "}
        {delta.newBytes === null ? (
          <UnknownValue />
        ) : (
          formatBytes(delta.newBytes)
        )}
      </span>
      <span className="delta-ledger__jump" aria-hidden="true">
        {t("history.delta.open")}
      </span>
    </>
  );

  if (!changed) {
    return <li className="delta-ledger__item delta-ledger__item--static">{content}</li>;
  }

  return (
    <li className="delta-ledger__item">
      <a
        className="delta-ledger__row"
        href="#storage"
        onClick={() => onOpenCategory(delta.categoryId)}
        aria-label={t("history.delta.aria", {
          name: delta.name,
          delta: formatDelta(delta.bytesDelta)
        })}
      >
        {content}
      </a>
    </li>
  );
}

function SnapshotRow({ snapshot, index }: { snapshot: HistorySnapshot; index: number }) {
  const partial = snapshot.coverage.completeness !== "complete";
  return (
    <li className="timeline__item">
      <span className="timeline__marker cs-num" aria-hidden="true">
        {String(index + 1).padStart(2, "0")}
      </span>
      <article className="timeline__body">
        <header>
          <div>
            <p className="cs-eyebrow">{t(TRIGGER_LABELS[snapshot.trigger])}</p>
            <h3>{formatDateTime(snapshot.takenAt)}</h3>
          </div>
          <StatusPill tone={partial ? "attention" : "positive"}>
            {t(partial ? "status.partial" : "status.complete")}
          </StatusPill>
        </header>
        <dl className="timeline__facts">
          <div>
            <dt>{t("history.snapshot.observedTotal")}</dt>
            <dd>
              <Bytes value={snapshot.totalBytes} partial={partial} />
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
              {snapshot.entries === null ? (
                <UnknownValue />
              ) : (
                <DataValue partial={partial}>
                  {tCount(snapshot.entries, "unit.entry.one", "unit.entry.other")}
                </DataValue>
              )}
            </dd>
          </div>
        </dl>
      </article>
    </li>
  );
}

function SystemRecords({ api }: { api: CoreSightApi }) {
  return (
    <section className="history-records" aria-labelledby="history-records-heading">
      <SectionHeading
        id="history-records-heading"
        title={t("history.records.title")}
        description={t("history.records.description")}
      />
      <div className="history-records__grid">
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
  const maxDelta = Math.max(
    ...(change?.categoryDeltas.map((delta) => Math.abs(delta.bytesDelta ?? 0)) ?? []),
    1
  );

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

      <CoverageBanner complete={!partial} coverage={data.coverage} context="scan" />

      {data.snapshots.length === 0 ? (
        <EmptyState
          title={t("history.empty.title")}
          body={t("history.empty.body")}
          next={t("history.empty.next")}
        />
      ) : (
        <>
          <Panel
            id="change-heading"
            title={t("history.change.title")}
            description={change?.note ?? t("history.change.descriptionFallback")}
            actions={
              canCompare ? (
                <StatusPill tone={partial ? "attention" : "positive"}>
                  {t(partial ? "status.partialComparison" : "status.comparableCoverage")}
                </StatusPill>
              ) : (
                <StatusPill tone="quiet">{t("status.baselineOnly")}</StatusPill>
              )
            }
          >
            {canCompare && change ? (
              <>
                <div className="change-hero">
                  <div className="change-hero__figure">
                    <span>{t("history.change.total")}</span>
                    <strong className="cs-num">
                      <DataValue partial={partial}>
                        {formatDelta(change.totalBytesDelta)}
                      </DataValue>
                    </strong>
                  </div>
                  <p className="change-hero__range">
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

                <div className="delta-ledger__head" aria-hidden="true">
                  <span>{t("storage.column.category")}</span>
                  <span />
                  <span>{t("storage.column.change")}</span>
                  <span>{t("history.change.title")}</span>
                  <span />
                </div>
                <ul className="delta-ledger">
                  {change.categoryDeltas.map((delta) => (
                    <DeltaRow
                      key={delta.categoryId}
                      delta={delta}
                      change={change}
                      maxDelta={maxDelta}
                      onOpenCategory={onOpenCategory}
                    />
                  ))}
                </ul>
              </>
            ) : (
              <div className="cs-unknown-block">
                <strong>{t("history.change.unknown")}</strong>
                <p>{t("history.change.unknownBody")}</p>
              </div>
            )}
          </Panel>

          <Panel
            id="timeline-heading"
            title={t("history.timeline.title")}
            description={t("history.timeline.description")}
          >
            <ol className="timeline">
              {data.snapshots.map((snapshot, index) => (
                <SnapshotRow snapshot={snapshot} index={index} key={snapshot.id} />
              ))}
            </ol>
          </Panel>
        </>
      )}

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
        layout="timeline"
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

  return <HistoryContent api={api} data={state.data} onOpenCategory={onOpenCategory} />;
}
