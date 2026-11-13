import type { CoreSightApi, Drive, DrivesData } from "../api";
import { useApiData } from "../hooks/useApiData";
import { formatDateTime, formatPercent } from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";
import {
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  SizeBar,
  StatusPill,
  UnknownValue,
  type PillTone
} from "../components/ui";

function healthPresentation(health: Drive["health"]): { label: MessageKey; tone: PillTone } {
  if (health === "healthy") return { label: "drive.health.healthy", tone: "positive" };
  if (health === "attention") return { label: "drive.health.attention", tone: "attention" };
  return { label: "drive.health.unknown", tone: "quiet" };
}

function DriveRow({ drive, partial }: { drive: Drive; partial: boolean }) {
  const health = healthPresentation(drive.health);
  const remembered = drive.connection === "remembered";
  const stateLabel = remembered
    ? t("drive.connection.rememberedDisconnected")
    : t("drive.connection.connected");

  return (
    <li className="drive-row">
      <article aria-labelledby={`drive-${drive.id}-name`}>
        <header className="drive-row__header">
          <div>
            <p className="eyebrow">
              {t(drive.kind === "external" ? "drive.kind.external" : "drive.kind.internal")}
            </p>
            <h3 id={`drive-${drive.id}-name`}>{drive.name}</h3>
          </div>
          <div className="drive-row__badges">
            <StatusPill tone={remembered ? "quiet" : "positive"}>{stateLabel}</StatusPill>
            <StatusPill tone={health.tone}>{t(health.label)}</StatusPill>
          </div>
        </header>

        <div className="drive-row__capacity">
          <div className="drive-row__capacity-label">
            <span>{t("drives.row.usedSpace")}</span>
            <DataValue partial={partial}>
              {drive.usedShare === null ? (
                <UnknownValue />
              ) : (
                formatPercent(drive.usedShare)
              )}
            </DataValue>
          </div>
          <SizeBar
            share={drive.usedShare}
            label={t("drives.row.usedSpaceLabel", { name: drive.name })}
            partial={partial}
          />
        </div>

        <dl className="drive-row__facts">
          <div>
            <dt>{t("drives.row.free")}</dt>
            <dd>
              <Bytes value={drive.freeBytes} partial={partial} />
            </dd>
          </div>
          <div>
            <dt>{t("drives.row.used")}</dt>
            <dd>
              <Bytes value={drive.usedBytes} partial={partial} />
            </dd>
          </div>
          <div>
            <dt>{t("drives.row.capacity")}</dt>
            <dd>
              <Bytes value={drive.totalBytes} partial={partial} />
            </dd>
          </div>
          <div>
            <dt>{t("drives.row.lastScan")}</dt>
            <dd>{formatDateTime(drive.lastScanAt)}</dd>
          </div>
          <div>
            <dt>{t("drives.row.snapshots")}</dt>
            <dd>
              <DataValue partial={partial}>
                {drive.snapshotCount === null
                  ? t("drives.row.snapshotsUnknown")
                  : tCount(drive.snapshotCount, "unit.snapshot.one", "unit.snapshot.other")}
              </DataValue>
            </dd>
          </div>
        </dl>

        {drive.note ? <p className="drive-row__note">{drive.note}</p> : null}
        {remembered ? <p className="microcopy">{t("drives.row.rememberedNote")}</p> : null}
      </article>
    </li>
  );
}

function DrivesContent({ data }: { data: DrivesData }) {
  const partial = data.coverage.completeness !== "complete";
  const connected = data.drives.filter((drive) => drive.connection === "connected").length;
  const remembered = data.drives.filter((drive) => drive.connection === "remembered").length;

  return (
    <div className="screen screen--drives">
      <PageHeader
        eyebrow={t("drives.eyebrow")}
        title={t("drives.title")}
        description={t("drives.description")}
        aside={
          <div className="snapshot-stamp">
            <span>{t("drives.stamp.title")}</span>
            <strong>{t("drives.stamp.body")}</strong>
            <span className="snapshot-stamp__note">{t("drives.stamp.note")}</span>
          </div>
        }
      />

      <CoverageBanner
        complete={!partial}
        coverage={data.coverage}
        context="scan"
      />

      {data.drives.length === 0 ? (
        <EmptyState
          title={t("drives.empty.title")}
          body={t("drives.empty.body")}
          next={t("drives.empty.next")}
        />
      ) : (
        <>
          <section className="drive-overview" aria-labelledby="drive-overview-title">
            <div>
              <p className="eyebrow">{t("drives.overview.title")}</p>
              <h2 id="drive-overview-title">
                <DataValue partial={partial}>
                  {tCount(data.drives.length, "unit.drive.one", "unit.drive.other")}
                </DataValue>
              </h2>
              <p>{t("drives.overview.body")}</p>
            </div>
            <dl className="summary-facts">
              <div>
                <dt>{t("drives.overview.connected")}</dt>
                <dd>
                  <DataValue partial={partial}>
                    {tCount(connected, "unit.drive.one", "unit.drive.other")}
                  </DataValue>
                </dd>
              </div>
              <div>
                <dt>{t("drives.overview.remembered")}</dt>
                <dd>
                  <DataValue partial={partial}>
                    {tCount(remembered, "unit.drive.one", "unit.drive.other")}
                  </DataValue>
                </dd>
              </div>
              <div>
                <dt>{t("drives.overview.writable")}</dt>
                <dd>
                  {partial
                    ? t("drives.overview.unavailablePartial")
                    : tCount(
                        connected,
                        "unit.connectedDrive.one",
                        "unit.connectedDrive.other"
                      )}
                </dd>
              </div>
            </dl>
          </section>

          <section className="panel drive-panel" aria-labelledby="drive-list-heading">
            <SectionHeading
              id="drive-list-heading"
              title={t("drives.list.title")}
              description={t("drives.list.description")}
            />
            <ul className="drive-list">
              {data.drives.map((drive) => (
                <DriveRow drive={drive} partial={partial} key={drive.id} />
              ))}
            </ul>
          </section>

          <section className="panel panel--soft drive-memory" aria-labelledby="drive-memory-heading">
            <SectionHeading
              id="drive-memory-heading"
              title={t("drives.memory.title")}
              description={t("drives.memory.description")}
            />
            <ul>
              <li>
                <strong>{t("drives.memory.connectedTerm")}</strong>{" "}
                {t("drives.memory.connectedBody")}
              </li>
              <li>
                <strong>{t("drives.memory.rememberedTerm")}</strong>{" "}
                {t("drives.memory.rememberedBody")}
              </li>
              <li>
                <strong>{t("drives.memory.unknownTerm")}</strong>{" "}
                {t("drives.memory.unknownBody")}
              </li>
            </ul>
          </section>
        </>
      )}
    </div>
  );
}

export function DrivesScreen({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.listDrives(), [api]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow={t("drives.eyebrow")}
        title={t("drives.title")}
        description={t("drives.description")}
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow={t("drives.eyebrow")}
        title={t("drives.title")}
        description={t("drives.description")}
        error={state.error}
      />
    );
  }

  return <DrivesContent data={state.data} />;
}
