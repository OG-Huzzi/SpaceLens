import type { CoreSightApi, Drive, DrivesData } from "../api";
import { useApiData } from "../hooks/useApiData";
import { formatBytes, formatCount, formatDateTime, formatPercent } from "../lib/format";
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

function healthPresentation(health: Drive["health"]): { label: string; tone: PillTone } {
  if (health === "healthy") return { label: "Healthy", tone: "positive" };
  if (health === "attention") return { label: "Needs attention", tone: "attention" };
  return { label: "Health unknown", tone: "quiet" };
}

function DriveRow({ drive, partial }: { drive: Drive; partial: boolean }) {
  const health = healthPresentation(drive.health);
  const remembered = drive.connection === "remembered";
  const stateLabel = remembered ? "Remembered · disconnected" : "Connected";

  return (
    <li className="drive-row">
      <article aria-labelledby={`drive-${drive.id}-name`}>
        <header className="drive-row__header">
          <div>
            <p className="eyebrow">
              {drive.kind === "external" ? "External drive" : "Internal drive"}
            </p>
            <h3 id={`drive-${drive.id}-name`}>{drive.name}</h3>
          </div>
          <div className="drive-row__badges">
            <StatusPill tone={remembered ? "quiet" : "positive"}>{stateLabel}</StatusPill>
            <StatusPill tone={health.tone}>{health.label}</StatusPill>
          </div>
        </header>

        <div className="drive-row__capacity">
          <div className="drive-row__capacity-label">
            <span>Used space</span>
            <DataValue partial={partial}>
              {drive.usedShare === null ? "Share unknown" : formatPercent(drive.usedShare)}
            </DataValue>
          </div>
          <SizeBar
            share={drive.usedShare}
            label={`${drive.name} used space`}
            partial={partial}
          />
        </div>

        <dl className="drive-row__facts">
          <div>
            <dt>Free</dt>
            <dd>
              <DataValue partial={partial}>{formatBytes(drive.freeBytes)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>Used</dt>
            <dd>
              <DataValue partial={partial}>{formatBytes(drive.usedBytes)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>Capacity</dt>
            <dd>
              <DataValue partial={partial}>{formatBytes(drive.totalBytes)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>Last scan</dt>
            <dd>{formatDateTime(drive.lastScanAt)}</dd>
          </div>
          <div>
            <dt>Snapshots</dt>
            <dd>
              <DataValue partial={partial}>
                {formatCount(drive.snapshotCount, "snapshot", "snapshots")}
              </DataValue>
            </dd>
          </div>
        </dl>

        {drive.note ? <p className="drive-row__note">{drive.note}</p> : null}
        {remembered ? (
          <p className="microcopy">
            Last-known figures come from this drive's previous scan. A disconnected drive is
            read-only and cannot be a cleanup target.
          </p>
        ) : null}
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
        eyebrow="Drives"
        title="What drives do I have?"
        description="Local and previously seen external drives, each with its connection state and last-known health."
        aside={
          <div className="snapshot-stamp">
            <span>Drive memory</span>
            <strong>Stored on this device</strong>
            <span className="snapshot-stamp__note">Offline records stay read-only</span>
          </div>
        }
      />

      <CoverageBanner coverage={data.coverage} />

      {data.drives.length === 0 ? (
        <EmptyState
          title="No drives recorded yet"
          description="CoreSight has no connected or remembered drive records. This is an empty inventory, not a zero-byte drive."
          nextStep="After the first completed scan, the drive and its capacity context will appear here."
        />
      ) : (
        <>
          <section className="drive-overview" aria-labelledby="drive-overview-title">
            <div>
              <p className="eyebrow">Drive inventory</p>
              <h2 id="drive-overview-title">
                <DataValue partial={partial}>
                  {formatCount(data.drives.length, "drive", "drives")}
                </DataValue>
              </h2>
              <p>Connected and remembered records share one list; their state is always labelled.</p>
            </div>
            <dl className="summary-facts">
              <div>
                <dt>Connected</dt>
                <dd>
                  <DataValue partial={partial}>
                    {formatCount(connected, "drive", "drives")}
                  </DataValue>
                </dd>
              </div>
              <div>
                <dt>Remembered</dt>
                <dd>
                  <DataValue partial={partial}>
                    {formatCount(remembered, "drive", "drives")}
                  </DataValue>
                </dd>
              </div>
              <div>
                <dt>Writable targets</dt>
                <dd>
                  {partial
                    ? "Unavailable on partial data"
                    : formatCount(connected, "connected drive", "connected drives")}
                </dd>
              </div>
            </dl>
          </section>

          <section className="panel drive-panel" aria-labelledby="drive-list-heading">
            <SectionHeading
              id="drive-list-heading"
              title="Drives"
              description="Capacity is logical observed storage. Free space can be unknown; it is never filled in with a guess."
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
              title="How drive memory works"
              description="Remembered drives are records, not live mounts."
            />
            <ul>
              <li>
                <strong>Connected</strong> means CoreSight can currently read the drive. Sizes and
                health are from the latest scan.
              </li>
              <li>
                <strong>Remembered</strong> means the drive was seen before and is detached now.
                Its figures are explicitly last-known and cannot be acted on.
              </li>
              <li>
                <strong>Unknown remains unknown.</strong> A disconnected drive's free space is not
                shown as zero, and stale health is never labelled healthy.
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
        eyebrow="Drives"
        title="What drives do I have?"
        description="Local and previously seen external drives, each with its connection state and last-known health."
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow="Drives"
        title="What drives do I have?"
        description="Local and previously seen external drives, each with its connection state and last-known health."
        error={state.error}
      />
    );
  }

  return <DrivesContent data={state.data} />;
}
