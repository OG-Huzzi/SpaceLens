import type {
  CoreSightApi,
  DataCoverage,
  DnsExcludedCandidate,
  DnsPrivacyPolicy,
  DnsRankedCandidate
} from "../api";
import { useApiData } from "../hooks/useApiData";
import { formatNumber, formatPercent } from "../lib/format";
import { getActiveLocale, t, tCount, type MessageKey } from "../i18n";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineNotice,
  SectionHeading
} from "../components/ui";
import { PanelFailure, PanelFrame, PanelLoading } from "./PanelFrame";

const PRIVACY_POLICY_LABEL: Record<DnsPrivacyPolicy, MessageKey> = {
  DOCUMENTED_NO_LOGGING: "dns.privacyPolicy.documentedNoLogging",
  DOCUMENTED_RETENTION: "dns.privacyPolicy.documentedRetention",
  UNDISCLOSED: "dns.privacyPolicy.undisclosed",
  UNKNOWN: "dns.privacyPolicy.unknown"
};

const EXCLUSION_LABEL: Record<DnsExcludedCandidate["reason"], MessageKey> = {
  INSUFFICIENT_EVIDENCE: "dns.exclusion.insufficientEvidence",
  TOO_UNRELIABLE: "dns.exclusion.tooUnreliable",
  UNDISCLOSED_PRIVACY: "dns.exclusion.undisclosedPrivacy",
  NOT_MEASURED: "dns.exclusion.notMeasured"
};

function scoreText(value: number | null): string {
  return value === null ? t("state.unknown") : t("dns.score.value", { value: formatNumber(value) });
}

function latencyText(value: number | null): string {
  if (value === null) return t("state.unknown");
  const formatted = new Intl.NumberFormat(getActiveLocale(), {
    maximumFractionDigits: 1
  }).format(value);
  return t("dns.milliseconds", { value: formatted });
}

function yesNo(value: boolean | null): string {
  if (value === null) return t("state.unknown");
  return t(value ? "dns.value.yes" : "dns.value.no");
}

function ScoreExplainer({
  label,
  weight,
  share,
  value,
  partial
}: {
  label: MessageKey;
  weight: MessageKey;
  share: number;
  value: number | null;
  partial: boolean;
}) {
  return (
    <li>
      <span className="evidence-list__kind" aria-hidden="true">
        {formatPercent(share)}
      </span>
      <div>
        <strong>{t(label)}</strong>
        <p>
          <DataValue partial={partial}>{scoreText(value)}</DataValue>
          {" · "}
          {t(weight)}
        </p>
      </div>
    </li>
  );
}

function RankedCandidate({
  candidate,
  index,
  partial
}: {
  candidate: DnsRankedCandidate;
  index: number;
  partial: boolean;
}) {
  return (
    <li className="cleanup-card">
      <div className="cleanup-card__header">
        <div className="cleanup-card__title">
          <p className="eyebrow">{t("dns.ranked.rank", { rank: index + 1 })}</p>
          <h3>{candidate.name}</h3>
          <p>{candidate.operator ?? t("dns.operatorUnknown")}</p>
        </div>
        <div className="cleanup-card__recoverable">
          <span>{t("dns.score.total")}</span>
          <DataValue partial={partial}>{scoreText(candidate.score)}</DataValue>
        </div>
      </div>

      <ul className="evidence-list" aria-label={t("dns.score.breakdownFor", { name: candidate.name })}>
        <ScoreExplainer
          label="dns.score.latency"
          weight="dns.score.latencyWeight"
          share={0.4}
          value={candidate.latencyScore}
          partial={partial}
        />
        <ScoreExplainer
          label="dns.score.reliability"
          weight="dns.score.reliabilityWeight"
          share={0.3}
          value={candidate.reliabilityScore}
          partial={partial}
        />
        <ScoreExplainer
          label="dns.score.dnssec"
          weight="dns.score.dnssecWeight"
          share={0.15}
          value={candidate.dnssecScore}
          partial={partial}
        />
        <ScoreExplainer
          label="dns.score.privacy"
          weight="dns.score.privacyWeight"
          share={0.15}
          value={candidate.privacyScore}
          partial={partial}
        />
      </ul>
      <p className="microcopy">{t("dns.score.explanation")}</p>

      <dl className="summary-facts">
        <div>
          <dt>{t("dns.fact.medianLatency")}</dt>
          <dd>{latencyText(candidate.medianLatencyMs)}</dd>
        </div>
        <div>
          <dt>{t("dns.fact.p95Latency")}</dt>
          <dd>{latencyText(candidate.p95LatencyMs)}</dd>
        </div>
        <div>
          <dt>{t("dns.fact.failureRate")}</dt>
          <dd>{formatPercent(candidate.failureRate)}</dd>
        </div>
        <div>
          <dt>{t("dns.fact.samples")}</dt>
          <dd>
            {candidate.samples === null
              ? t("state.unknown")
              : tCount(candidate.samples, "unit.sample.one", "unit.sample.other")}
          </dd>
        </div>
        <div>
          <dt>{t("dns.fact.privacyPolicy")}</dt>
          <dd>{t(PRIVACY_POLICY_LABEL[candidate.privacyPolicy])}</dd>
        </div>
        <div>
          <dt>{t("dns.fact.dnssec")}</dt>
          <dd>{yesNo(candidate.supportsDnssec)}</dd>
        </div>
        <div>
          <dt>{t("dns.fact.filtering")}</dt>
          <dd>{yesNo(candidate.supportsFiltering)}</dd>
        </div>
      </dl>
    </li>
  );
}

function DnsContent({
  ranked,
  excluded,
  note,
  coverage
}: {
  ranked: DnsRankedCandidate[];
  excluded: DnsExcludedCandidate[];
  note: string;
  coverage: DataCoverage;
}) {
  const partial = coverage.completeness !== "complete";

  if (ranked.length === 0 && excluded.length === 0) {
    return (
      <>
        <CoverageBanner coverage={coverage} />
        <EmptyState
          title={t("dns.empty.title")}
          description={t("dns.empty.body")}
          nextStep={t("dns.empty.next")}
        />
      </>
    );
  }

  return (
    <>
      <CoverageBanner coverage={coverage} />

      <InlineNotice tone="quiet">
        <strong>{t("dns.basis.title")}</strong>
        <p>{note}</p>
      </InlineNotice>

      <section aria-labelledby="dns-ranked-heading">
        <SectionHeading
          level={3}
          id="dns-ranked-heading"
          title={t("dns.ranked.title")}
          description={t("dns.ranked.description")}
          aside={
            <span className="eyebrow">
              {tCount(ranked.length, "unit.dnsCandidate.one", "unit.dnsCandidate.other")}
            </span>
          }
        />
        {ranked.length === 0 ? (
          <p className="microcopy">{t("dns.ranked.empty")}</p>
        ) : (
          <ul className="cleanup-list">
            {ranked.map((candidate, index) => (
              <RankedCandidate
                candidate={candidate}
                index={index}
                partial={partial}
                key={candidate.name}
              />
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="dns-excluded-heading">
        <SectionHeading
          level={3}
          id="dns-excluded-heading"
          title={t("dns.excluded.title")}
          description={t("dns.excluded.description")}
          aside={
            <span className="eyebrow">
              {tCount(excluded.length, "unit.dnsCandidate.one", "unit.dnsCandidate.other")}
            </span>
          }
        />
        {excluded.length === 0 ? (
          <p className="microcopy">{t("dns.excluded.empty")}</p>
        ) : (
          <ul className="evidence-list" aria-label={t("dns.excluded.title")}>
            {excluded.map((candidate) => (
              <li key={candidate.name}>
                <span className="evidence-list__kind">
                  {t(EXCLUSION_LABEL[candidate.reason])}
                </span>
                <div>
                  <strong>{candidate.name}</strong>
                  <p>{candidate.detail}</p>
                  <p className="microcopy">
                    {t("dns.exclusion.code")} <code>{candidate.reason}</code>
                  </p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

export function DnsRankingPanel({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getDnsRanking(), [api]);

  return (
    <PanelFrame
      id="dns-panel-heading"
      title={t("dns.title")}
      description={t("dns.description")}
    >
      <InlineNotice tone="attention">
        <strong>{t("dns.warning.title")}</strong>
        <p>{t("dns.warning.body")}</p>
      </InlineNotice>

      {state.status === "loading" ? <PanelLoading label={t("dns.loading")} /> : null}
      {state.status === "error" ? <PanelFailure error={state.error} /> : null}
      {state.status === "ready" ? (
        <DnsContent
          ranked={state.data.ranked}
          excluded={state.data.excluded}
          note={state.data.note}
          coverage={state.data.coverage}
        />
      ) : null}
    </PanelFrame>
  );
}
