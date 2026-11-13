import type { BlockListEntry, CoreSightApi, DataCoverage } from "../api";
import { useApiData } from "../hooks/useApiData";
import { t, tCount, type MessageKey } from "../i18n";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineNotice,
  SectionHeading
} from "../components/ui";
import { PanelFailure, PanelFrame, PanelLoading } from "./PanelFrame";

interface CategoryGroup {
  category: string;
  entries: BlockListEntry[];
}

function groupByCategory(entries: BlockListEntry[]): CategoryGroup[] {
  const groups = new Map<string, BlockListEntry[]>();
  for (const entry of entries) {
    const existing = groups.get(entry.category);
    if (existing === undefined) {
      groups.set(entry.category, [entry]);
    } else {
      existing.push(entry);
    }
  }
  return [...groups.entries()]
    .map(([category, groupedEntries]) => ({ category, entries: groupedEntries }))
    .sort((left, right) => left.category.localeCompare(right.category));
}

function counted(value: number | null, one: MessageKey, other: MessageKey): string {
  if (value === null) return t("state.unknown");
  return tCount(value, one, other);
}

function PlanFact({
  label,
  value,
  partial
}: {
  label: MessageKey;
  value: string;
  partial: boolean;
}) {
  return (
    <div>
      <dt>{t(label)}</dt>
      <dd>
        <DataValue partial={partial}>{value}</DataValue>
      </dd>
    </div>
  );
}

function BlockListContent({
  entries,
  total,
  duplicatesRemoved,
  invalidRemoved,
  whitelisted,
  truncated,
  note,
  coverage
}: {
  entries: BlockListEntry[];
  total: number | null;
  duplicatesRemoved: number | null;
  invalidRemoved: Array<{ domain: string; reason: string }>;
  whitelisted: string[];
  truncated: number | null;
  note: string;
  coverage: DataCoverage;
}) {
  const partial = coverage.completeness !== "complete";
  const groups = groupByCategory(entries);
  const knownEmpty =
    entries.length === 0 &&
    invalidRemoved.length === 0 &&
    whitelisted.length === 0 &&
    total === 0 &&
    duplicatesRemoved === 0 &&
    truncated === 0;

  if (knownEmpty) {
    return (
      <>
        <CoverageBanner coverage={coverage} />
        <EmptyState
          title={t("blocklist.empty.title")}
          description={t("blocklist.empty.body")}
          nextStep={t("blocklist.empty.next")}
        />
      </>
    );
  }

  return (
    <>
      <CoverageBanner coverage={coverage} />

      <dl className="summary-facts">
        <PlanFact
          label="blocklist.summary.total"
          value={counted(total, "blocklist.value.total.one", "blocklist.value.total.other")}
          partial={partial}
        />
        <PlanFact
          label="blocklist.summary.shown"
          value={counted(
            entries.length,
            "blocklist.value.total.one",
            "blocklist.value.total.other"
          )}
          partial={partial}
        />
        <PlanFact
          label="blocklist.summary.duplicates"
          value={counted(
            duplicatesRemoved,
            "blocklist.value.duplicates.one",
            "blocklist.value.duplicates.other"
          )}
          partial={partial}
        />
        <PlanFact
          label="blocklist.summary.invalid"
          value={counted(
            invalidRemoved.length,
            "blocklist.value.invalid.one",
            "blocklist.value.invalid.other"
          )}
          partial={partial}
        />
        <PlanFact
          label="blocklist.summary.whitelisted"
          value={counted(
            whitelisted.length,
            "blocklist.value.whitelisted.one",
            "blocklist.value.whitelisted.other"
          )}
          partial={partial}
        />
        <PlanFact
          label="blocklist.summary.truncated"
          value={counted(
            truncated,
            "blocklist.value.truncated.one",
            "blocklist.value.truncated.other"
          )}
          partial={partial}
        />
      </dl>

      <p className="microcopy">
        {t("blocklist.engineNote")} {note}
      </p>

      <section aria-labelledby="blocklist-groups-heading">
        <SectionHeading
          level={3}
          id="blocklist-groups-heading"
          title={t("blocklist.groups.title")}
          description={t("blocklist.groups.description")}
          aside={
            <span className="eyebrow">
              {counted(entries.length, "unit.blockEntry.one", "unit.blockEntry.other")}
            </span>
          }
        />
        {groups.length === 0 ? (
          <p className="microcopy">{t("blocklist.groups.empty")}</p>
        ) : (
          groups.map((group, index) => (
            <section
              aria-labelledby={`blocklist-group-${index}`}
              key={group.category}
            >
              <div className="section-heading">
                <div>
                  <h4 id={`blocklist-group-${index}`}>{group.category}</h4>
                </div>
                <div className="section-heading__aside">
                  <span className="eyebrow">
                    {counted(
                      group.entries.length,
                      "unit.blockEntry.one",
                      "unit.blockEntry.other"
                    )}
                  </span>
                </div>
              </div>
              <ul className="preview-items">
                {group.entries.map((entry) => (
                  <li key={`${group.category}-${entry.domain}`}>
                    <div>
                      <strong>{entry.domain}</strong>
                      <p>
                        {t("blocklist.entry.targets", {
                          ipv4: entry.ipv4,
                          ipv6: entry.ipv6
                        })}
                      </p>
                    </div>
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
      </section>

      <section aria-labelledby="blocklist-invalid-heading">
        <SectionHeading
          level={3}
          id="blocklist-invalid-heading"
          title={t("blocklist.invalid.title")}
          description={t("blocklist.invalid.description")}
          aside={
            <span className="eyebrow">
              {counted(
                invalidRemoved.length,
                "blocklist.value.invalid.one",
                "blocklist.value.invalid.other"
              )}
            </span>
          }
        />
        {invalidRemoved.length === 0 ? (
          <p className="microcopy">{t("blocklist.invalid.empty")}</p>
        ) : (
          <ul className="evidence-list" aria-label={t("blocklist.invalid.title")}>
            {invalidRemoved.map((entry) => (
              <li key={`${entry.domain}-${entry.reason}`}>
                <span className="evidence-list__kind">{t("blocklist.invalid.kind")}</span>
                <div>
                  <strong>{entry.domain}</strong>
                  <p>{entry.reason}</p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="blocklist-whitelist-heading">
        <SectionHeading
          level={3}
          id="blocklist-whitelist-heading"
          title={t("blocklist.whitelist.title")}
          description={t("blocklist.whitelist.description")}
          aside={
            <span className="eyebrow">
              {counted(
                whitelisted.length,
                "blocklist.value.whitelisted.one",
                "blocklist.value.whitelisted.other"
              )}
            </span>
          }
        />
        {whitelisted.length === 0 ? (
          <p className="microcopy">{t("blocklist.whitelist.empty")}</p>
        ) : (
          <ul className="evidence-list" aria-label={t("blocklist.whitelist.title")}>
            {whitelisted.map((domain) => (
              <li key={domain}>
                <span className="evidence-list__kind">
                  {t("blocklist.whitelist.kind")}
                </span>
                <div>
                  <strong>{domain}</strong>
                  <p>{t("blocklist.whitelist.item")}</p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

export function BlockListPanel({ api }: { api: CoreSightApi }) {
  const state = useApiData(() => api.getBlockListPlan(), [api]);

  return (
    <PanelFrame
      id="blocklist-panel-heading"
      title={t("blocklist.title")}
      description={t("blocklist.description")}
    >
      <InlineNotice tone="attention">
        <strong>{t("blocklist.planNotice.title")}</strong>
        <p>{t("blocklist.planNotice.body")}</p>
        <p>{t("blocklist.planNotice.reversible")}</p>
      </InlineNotice>

      {state.status === "loading" ? <PanelLoading label={t("blocklist.loading")} /> : null}
      {state.status === "error" ? <PanelFailure error={state.error} /> : null}
      {state.status === "ready" ? (
        <BlockListContent
          entries={state.data.entries}
          total={state.data.total}
          duplicatesRemoved={state.data.duplicatesRemoved}
          invalidRemoved={state.data.invalidRemoved}
          whitelisted={state.data.whitelisted}
          truncated={state.data.truncated}
          note={state.data.note}
          coverage={state.data.coverage}
        />
      ) : null}
    </PanelFrame>
  );
}
