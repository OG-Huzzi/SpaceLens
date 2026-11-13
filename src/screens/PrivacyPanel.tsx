import type {
  CoreSightApi,
  DataCoverage,
  PrivacyFinding,
  PrivacyFindingKind
} from "../api";
import { useApiData } from "../hooks/useApiData";
import { formatBytes } from "../lib/format";
import { t, tCount, type MessageKey } from "../i18n";
import {
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineNotice,
  SectionHeading,
  UnknownValue
} from "../components/ui";
import { PanelFailure, PanelFrame, PanelLoading } from "./PanelFrame";

const FINDING_KIND: Record<PrivacyFindingKind, MessageKey> = {
  TRACKING_COOKIE: "privacy.kind.trackingCookie",
  SESSION_COOKIE: "privacy.kind.sessionCookie",
  BROWSING_HISTORY: "privacy.kind.browsingHistory",
  CACHE_ENTRY: "privacy.kind.cacheEntry",
  SITE_STORAGE: "privacy.kind.siteStorage",
  UNKNOWN: "privacy.kind.unknown"
};

function FindingRow({ finding, partial }: { finding: PrivacyFinding; partial: boolean }) {
  return (
    <li>
      <span className="evidence-list__kind">{t(FINDING_KIND[finding.kind])}</span>
      <div>
        <strong>
          <DataValue partial={partial}>
            {finding.count === null
              ? t("privacy.finding.countUnknown")
              : tCount(finding.count, "unit.finding.one", "unit.finding.other")}
          </DataValue>
        </strong>
        <p>{finding.detail}</p>
        <p className="microcopy">
          {t("privacy.finding.estimatedBytes")}{" "}
          {finding.estimatedBytes === null ? (
            <UnknownValue label={t("privacy.finding.estimateUnknown")} />
          ) : (
            <DataValue partial={partial}>{formatBytes(finding.estimatedBytes)}</DataValue>
          )}
        </p>
      </div>
    </li>
  );
}

function PrivacyContent({
  findings,
  totalCount,
  totalEstimatedBytes,
  hasUnknownKinds,
  note,
  coverage
}: {
  findings: PrivacyFinding[];
  totalCount: number | null;
  totalEstimatedBytes: number | null;
  hasUnknownKinds: boolean;
  note: string;
  coverage: DataCoverage;
}) {
  const partial = coverage.completeness !== "complete";
  const sessionFindings = findings.filter((finding) => finding.kind === "SESSION_COOKIE");
  const knownEmpty =
    findings.length === 0 && totalCount === 0 && totalEstimatedBytes === 0;

  if (knownEmpty) {
    return (
      <>
        <CoverageBanner
          complete={!partial}
          coverage={coverage}
          context="scan"
        />
        <EmptyState
          title={t("privacy.empty.title")}
          body={t("privacy.empty.body")}
          next={t("privacy.empty.next")}
        />
      </>
    );
  }

  return (
    <>
      <CoverageBanner complete={!partial} coverage={coverage} context="scan" />

      {sessionFindings.length > 0 ? (
        <InlineNotice tone="attention">
          <strong>{t("privacy.sessionWarning.title")}</strong>
          <p>{t("privacy.sessionWarning.body")}</p>
          {sessionFindings.map((finding, index) => (
            <p key={`${finding.kind}-${index}`}>{finding.detail}</p>
          ))}
        </InlineNotice>
      ) : null}

      <dl className="summary-facts">
        <div>
          <dt>{t("privacy.summary.total")}</dt>
          <dd>
            <DataValue partial={partial}>
              {totalCount === null
                ? t("state.unknown")
                : tCount(totalCount, "unit.finding.one", "unit.finding.other")}
            </DataValue>
          </dd>
        </div>
        <div>
          <dt>{t("privacy.summary.bytes")}</dt>
          <dd>
            <Bytes value={totalEstimatedBytes} partial={partial} />
          </dd>
        </div>
        <div>
          <dt>{t("privacy.summary.unknownKinds")}</dt>
          <dd>{t(hasUnknownKinds ? "privacy.value.yes" : "privacy.value.no")}</dd>
        </div>
      </dl>

      {totalEstimatedBytes === null ? (
        <p className="microcopy">{t("privacy.totalUnknownReason")}</p>
      ) : null}
      <p className="microcopy">{t("privacy.estimateNote")}</p>

      {hasUnknownKinds ? (
        <InlineNotice tone="quiet">
          <strong>{t("privacy.unknownKinds.noticeTitle")}</strong>
          <p>{t("privacy.unknownKinds.notice")}</p>
        </InlineNotice>
      ) : null}

      <section aria-labelledby="privacy-findings-heading">
        <SectionHeading
          level={3}
          id="privacy-findings-heading"
          title={t("privacy.findings.title")}
          description={t("privacy.findings.description")}
          aside={
            <span className="eyebrow">
              {tCount(findings.length, "unit.finding.one", "unit.finding.other")}
            </span>
          }
        />
        <ul className="evidence-list" aria-label={t("privacy.findings.title")}>
          {findings.map((finding, index) => (
            <FindingRow
              finding={finding}
              partial={partial}
              key={`${finding.kind}-${index}`}
            />
          ))}
        </ul>
      </section>

      <p className="microcopy">
        {t("privacy.engineNote")} {note}
      </p>
    </>
  );
}

export function PrivacyPanel({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getPrivacySummary(), [api]);

  return (
    <PanelFrame
      id="privacy-panel-heading"
      title={t("privacy.title")}
      description={t("privacy.description")}
    >
      {state.status === "loading" ? <PanelLoading label={t("privacy.loading")} /> : null}
      {state.status === "error" ? <PanelFailure error={state.error} /> : null}
      {state.status === "ready" ? (
        <PrivacyContent
          findings={state.data.findings}
          totalCount={state.data.totalCount}
          totalEstimatedBytes={state.data.totalEstimatedBytes}
          hasUnknownKinds={state.data.hasUnknownKinds}
          note={state.data.note}
          coverage={state.data.coverage}
        />
      ) : null}
    </PanelFrame>
  );
}
