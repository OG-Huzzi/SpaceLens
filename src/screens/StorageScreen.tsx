import { useState } from "react";
import type { CategoryTotals, CoreSightApi, StorageData } from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatAge,
  formatBytes,
  formatDateTime,
  formatDelta,
  formatPercent
} from "../lib/format";
import { t, tCount } from "../i18n";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  PageHeader,
  PartialTag,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  SizeBar,
  StatusPill
} from "../components/ui";

function categoryIsPartial(category: CategoryTotals, overallPartial: boolean): boolean {
  return (
    overallPartial ||
    category.entriesWithoutSize === null ||
    category.entriesWithoutSize > 0 ||
    category.observationErrors === null ||
    category.observationErrors > 0
  );
}

function containsCategory(category: CategoryTotals, id: string): boolean {
  return (
    category.id === id || category.children.some((child) => containsCategory(child, id))
  );
}

function filterCategory(category: CategoryTotals, query: string): CategoryTotals | null {
  if (category.name.toLocaleLowerCase().includes(query)) return category;
  const children = category.children
    .map((child) => filterCategory(child, query))
    .filter((child): child is CategoryTotals => child !== null);
  if (children.length === 0) return null;
  return { ...category, children };
}

function entryCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.entry.one", "unit.entry.other");
}

function observedEntryCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.observedEntry.one", "unit.observedEntry.other");
}

function CategoryRow({
  category,
  depth,
  overallPartial,
  forceOpen,
  focusCategoryId
}: {
  category: CategoryTotals;
  depth: number;
  overallPartial: boolean;
  forceOpen: boolean;
  focusCategoryId: string | null;
}) {
  const partial = categoryIsPartial(category, overallPartial);
  const shouldOpen = forceOpen || (focusCategoryId !== null && containsCategory(category, focusCategoryId));
  const isFocused = category.id === focusCategoryId;
  const knownChildren = category.children.length;
  const childrenLabel =
    knownChildren > 0
      ? tCount(knownChildren, "unit.subcategory.one", "unit.subcategory.other")
      : t("storage.category.noDeeperBreakdown");
  const summaryLabel = t("storage.category.summary", {
    name: category.name,
    bytes: formatBytes(category.bytes),
    percent: formatPercent(category.shareOfParent),
    delta: formatDelta(category.deltaSinceLast),
    partial: partial ? t("state.sizeBar.partialSuffix") : "",
    children: childrenLabel
  });

  return (
    <details
      className={`category category--depth-${Math.min(depth, 2)}${isFocused ? " category--focused" : ""}`}
      open={shouldOpen || undefined}
    >
      <summary className="category-summary" aria-label={summaryLabel}>
        <span className="category-summary__chevron" aria-hidden="true" />
        <span className="category-summary__name">
          <span className="category-summary__title">{category.name}</span>
          <span className="category-summary__meta">{childrenLabel}</span>
        </span>
        <SizeBar
          share={category.shareOfParent}
          label={t("storage.category.shareLabel", { name: category.name })}
          partial={partial}
        />
        <span className="category-summary__bytes">
          <DataValue partial={partial}>{formatBytes(category.bytes)}</DataValue>
        </span>
        <span className="category-summary__share">
          {formatPercent(category.shareOfParent)}
        </span>
        <span
          className={`category-summary__delta${
            category.deltaSinceLast !== null && category.deltaSinceLast > 0
              ? " delta delta--up"
              : " delta"
          }`}
        >
          {formatDelta(category.deltaSinceLast)}
        </span>
      </summary>

      <div className="category-detail">
        <dl className="category-facts">
          <div>
            <dt>{t("storage.category.entries")}</dt>
            <dd>
              <DataValue partial={partial}>{entryCount(category.itemCount)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>{t("storage.category.entriesWithoutSize")}</dt>
            <dd>
              <DataValue partial={partial}>{entryCount(category.entriesWithoutSize)}</DataValue>
            </dd>
          </div>
          <div>
            <dt>{t("storage.category.observationErrors")}</dt>
            <dd>
              <DataValue partial={partial}>{entryCount(category.observationErrors)}</DataValue>
            </dd>
          </div>
        </dl>

        {category.children.length > 0 ? (
          <ul className="category-children">
            {category.children.map((child) => (
              <li key={child.id}>
                <CategoryRow
                  category={child}
                  depth={depth + 1}
                  overallPartial={overallPartial}
                  forceOpen={forceOpen}
                  focusCategoryId={focusCategoryId}
                />
              </li>
            ))}
          </ul>
        ) : (
          <p className="microcopy">{t("storage.category.noDeeperBody")}</p>
        )}

        {category.sampleLocations.length > 0 ? (
          <details className="location-disclosure">
            <summary
              aria-label={t("storage.category.sampleLocationsAria", { name: category.name })}
            >
              <span>{t("storage.category.sampleLocations")}</span>
              <span className="location-disclosure__count">
                {tCount(category.sampleLocations.length, "unit.label.one", "unit.label.other")}
              </span>
            </summary>
            <p className="microcopy">{t("storage.category.sampleLocationsNote")}</p>
            <dl className="location-list">
              {category.sampleLocations.map((location) => (
                <div key={`${category.id}-${location.displayPath}`}>
                  <dt>{location.label}</dt>
                  <dd>
                    <code>{location.displayPath}</code>
                    <DataValue partial={partial}>{formatBytes(location.bytes)}</DataValue>
                  </dd>
                </div>
              ))}
            </dl>
          </details>
        ) : null}
      </div>
    </details>
  );
}

function StorageContent({
  data,
  focusCategoryId
}: {
  data: StorageData;
  focusCategoryId: string | null;
}) {
  const [query, setQuery] = useState("");
  const normalisedQuery = query.trim().toLocaleLowerCase();
  const partial = data.coverage.completeness !== "complete";

  if (data.categories.length === 0) {
    return (
      <div className="screen">
        <PageHeader
          eyebrow={t("storage.eyebrow")}
          title={t("storage.title")}
          description={t("storage.description")}
        />
        <CoverageBanner coverage={data.coverage} />
        <EmptyState
          title={t("storage.empty.title")}
          description={t("storage.empty.body")}
          nextStep={t("storage.empty.next")}
        />
      </div>
    );
  }

  const filteredCategories = data.categories
    .map((category) => filterCategory(category, normalisedQuery))
    .filter((category): category is CategoryTotals => category !== null);
  const matchingTopLevel = normalisedQuery.length > 0 ? filteredCategories.length : data.categories.length;

  return (
    <div className="screen screen--storage">
      <PageHeader
        eyebrow={t("storage.eyebrow")}
        title={t("storage.title")}
        description={t("storage.descriptionMain")}
      />

      <CoverageBanner coverage={data.coverage} />

      <section className="storage-summary" aria-labelledby="storage-summary-title">
        <div className="storage-summary__lead">
          <p className="eyebrow">{t("storage.summary.observedTotal")}</p>
          <h2 id="storage-summary-title">
            <DataValue partial={partial}>{formatBytes(data.totalObservedBytes)}</DataValue>
          </h2>
          <p>
            {t(partial ? "storage.summary.bodyPartial" : "storage.summary.bodyComplete", {
              count: observedEntryCount(data.totalObservedItems)
            })}
          </p>
        </div>
        <dl className="summary-facts">
          <div>
            <dt>{t("storage.summary.categories")}</dt>
            <dd>
              <DataValue partial={partial}>
                {tCount(data.categories.length, "unit.category.one", "unit.category.other")}
              </DataValue>
            </dd>
          </div>
          <div>
            <dt>{t("storage.summary.entriesWithoutSize")}</dt>
            <dd>
              {entryCount(data.coverage.unknownSizeEntries)}
              {partial ? <PartialTag /> : null}
            </dd>
          </div>
          <div>
            <dt>{t("storage.summary.duplicateEvidence")}</dt>
            <dd>
              {t(
                data.duplicateReport.coverage.completeness === "complete"
                  ? "status.complete"
                  : "status.partial"
              )}
            </dd>
          </div>
        </dl>
      </section>

      <section className="panel storage-categories" aria-labelledby="storage-categories-heading">
        <SectionHeading
          id="storage-categories-heading"
          title={t("storage.categories.title")}
          description={t("storage.categories.description")}
          aside={
            <span className="eyebrow">
              <DataValue partial={partial}>
                {tCount(matchingTopLevel, "unit.topLevelGroup.one", "unit.topLevelGroup.other")}
              </DataValue>
            </span>
          }
        />

        <div className="storage-toolbar">
          <label className="search-field" htmlFor="storage-search">
            <span>{t("storage.search.label")}</span>
            <input
              id="storage-search"
              type="search"
              value={query}
              aria-label={t("storage.search.aria")}
              aria-describedby="storage-search-help"
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("storage.search.placeholder")}
              autoComplete="off"
            />
          </label>
          <p className="storage-toolbar__status" id="storage-search-help" aria-live="polite">
            {normalisedQuery.length === 0
              ? t("storage.search.help")
              : t("storage.search.status", {
                  matches: tCount(
                    filteredCategories.length,
                    "unit.match.one",
                    "unit.match.other"
                  ),
                  query: query.trim()
                })}
          </p>
        </div>

        {filteredCategories.length === 0 ? (
          <EmptyState
            title={t("storage.search.emptyTitle")}
            description={t("storage.search.emptyBody", { query: query.trim() })}
            nextStep={t("storage.search.emptyNext")}
          />
        ) : (
          <>
            <div className="category-columns" aria-hidden="true">
              <span />
              <span>{t("storage.column.category")}</span>
              <span>{t("storage.column.shareOfTotal")}</span>
              <span>{t("storage.column.observed")}</span>
              <span>{t("storage.column.share")}</span>
              <span>{t("storage.column.change")}</span>
            </div>
            <ul className="category-list">
              {filteredCategories.map((category) => (
                <li key={category.id}>
                  <CategoryRow
                    category={category}
                    depth={0}
                    overallPartial={partial}
                    forceOpen={normalisedQuery.length > 0}
                    focusCategoryId={focusCategoryId}
                  />
                </li>
              ))}
            </ul>
          </>
        )}
      </section>

      <div className="evidence-grid">
        <section className="panel evidence-panel" aria-labelledby="duplicate-heading">
          <SectionHeading
            id="duplicate-heading"
            title={t("storage.duplicates.title")}
            description={t("storage.duplicates.description")}
          />
          <CoverageBanner
            coverage={data.duplicateReport.coverage}
            context="duplicateAnalysis"
          />
          <div className="evidence-panel__summary">
            <span>{t("storage.duplicates.recoverable")}</span>
            <DataValue partial={data.duplicateReport.coverage.completeness !== "complete"}>
              {formatBytes(data.duplicateReport.totalRecoverableBytes)}
            </DataValue>
          </div>
          {data.duplicateReport.groups.length === 0 ? (
            <p className="microcopy">{t("storage.duplicates.empty")}</p>
          ) : (
            <div className="duplicate-list">
              {data.duplicateReport.groups.map((group, index) => (
                <article className="duplicate-group" key={group.id}>
                  <header>
                    <div>
                      <p className="eyebrow">
                        {t("storage.duplicates.group", {
                          index: String(index + 1).padStart(2, "0")
                        })}
                      </p>
                      <h3>
                        {t("storage.duplicates.groupHeading", {
                          count: tCount(
                            group.members.length,
                            "unit.verifiedCopy.one",
                            "unit.verifiedCopy.other"
                          ),
                          bytes: formatBytes(group.sizeBytes)
                        })}
                      </h3>
                      <p className="duplicate-group__recoverable">
                        <DataValue
                          partial={data.duplicateReport.coverage.completeness !== "complete"}
                        >
                          {t("storage.duplicates.recoverableValue", {
                            bytes: formatBytes(group.recoverableBytes)
                          })}
                        </DataValue>
                      </p>
                    </div>
                    <StatusPill tone="positive">{t("storage.duplicates.verified")}</StatusPill>
                  </header>
                  <p className="hash-line">
                    {t("storage.duplicates.hashLabel")}{" "}
                    <code>{group.contentSha256.slice(0, 12)}…</code>
                  </p>
                  <details className="member-disclosure">
                    <summary
                      aria-label={t("storage.duplicates.showMembersAria", {
                        index: index + 1
                      })}
                    >
                      <span>{t("storage.duplicates.showMembers")}</span>
                      <span>
                        {tCount(group.members.length, "unit.member.one", "unit.member.other")}
                      </span>
                    </summary>
                    <ul className="duplicate-members">
                      {group.members.map((member) => (
                        <li key={`${group.id}-${member.displayPath}`}>
                          <code>{member.displayPath}</code>
                          <span>
                            {formatBytes(member.sizeBytes)}
                            {member.isHardlinkAlias
                              ? t("storage.duplicates.hardlinkAlias")
                              : ""}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </details>
                  {group.members.some((member) => member.isHardlinkAlias) ? (
                    <p className="microcopy">{t("storage.duplicates.hardlinkNote")}</p>
                  ) : null}
                </article>
              ))}
            </div>
          )}
          {data.duplicateReport.unverifiedEntries !== null &&
          data.duplicateReport.unverifiedEntries > 0 ? (
            <p className="microcopy">
              <DataValue
                partial={data.duplicateReport.coverage.completeness !== "complete"}
              >
                {tCount(
                  data.duplicateReport.unverifiedEntries,
                  "storage.duplicates.unverified.one",
                  "storage.duplicates.unverified.other"
                )}
              </DataValue>
            </p>
          ) : null}
          {data.duplicateReport.unknownSizeEntries !== null &&
          data.duplicateReport.unknownSizeEntries > 0 ? (
            <p className="microcopy">
              <DataValue
                partial={data.duplicateReport.coverage.completeness !== "complete"}
              >
                {tCount(
                  data.duplicateReport.unknownSizeEntries,
                  "storage.duplicates.unknownSize.one",
                  "storage.duplicates.unknownSize.other"
                )}
              </DataValue>
            </p>
          ) : null}
        </section>

        <section className="panel evidence-panel" aria-labelledby="big-files-heading">
          <SectionHeading
            id="big-files-heading"
            title={t("storage.bigFiles.title")}
            description={t("storage.bigFiles.description", {
              bytes: formatBytes(data.bigFileReport.thresholdBytes)
            })}
          />
          <CoverageBanner coverage={data.bigFileReport.coverage} context="largeFileResult" />
          {data.bigFileReport.files.length === 0 ? (
            <p className="microcopy">{t("storage.bigFiles.empty")}</p>
          ) : (
            <ul className="big-file-list">
              {data.bigFileReport.files.map((file) => (
                <li key={file.id}>
                  <div className="big-file-list__headline">
                    <strong>
                      <DataValue
                        partial={data.bigFileReport.coverage.completeness !== "complete"}
                      >
                        {formatBytes(file.sizeBytes)}
                      </DataValue>
                    </strong>
                    <span>{file.categoryName ?? t("storage.bigFiles.categoryUnknown")}</span>
                  </div>
                  <div className="big-file-list__meta">
                    <span>
                      {file.ageDays === null
                        ? t("format.ageUnknown")
                        : t("storage.bigFiles.age", { age: formatAge(file.ageDays) })}
                    </span>
                    <span>
                      {file.modifiedAt === null
                        ? t("storage.bigFiles.modifiedUnknown")
                        : t("storage.bigFiles.modified", {
                            time: formatDateTime(file.modifiedAt)
                          })}
                    </span>
                  </div>
                  <details className="file-location">
                    <summary
                      aria-label={t("storage.bigFiles.showLocationAria", {
                        bytes: formatBytes(file.sizeBytes)
                      })}
                    >
                      {t("storage.bigFiles.showLocation")}
                    </summary>
                    <code>{file.displayPath}</code>
                  </details>
                </li>
              ))}
            </ul>
          )}
          {data.bigFileReport.unknownAgeEntries !== null &&
          data.bigFileReport.unknownAgeEntries > 0 ? (
            <p className="microcopy">
              <DataValue
                partial={data.bigFileReport.coverage.completeness !== "complete"}
              >
                {tCount(
                  data.bigFileReport.unknownAgeEntries,
                  "storage.bigFiles.unknownAge.one",
                  "storage.bigFiles.unknownAge.other"
                )}
              </DataValue>
            </p>
          ) : null}
        </section>
      </div>
    </div>
  );
}

export function StorageScreen({
  api,
  focusCategoryId
}: {
  api: CoreSightApi;
  focusCategoryId: string | null;
}) {
  const state = useApiData(() => api.getStorage(), [api]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow={t("storage.eyebrow")}
        title={t("storage.title")}
        description={t("storage.description")}
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow={t("storage.eyebrow")}
        title={t("storage.title")}
        description={t("storage.description")}
        error={state.error}
      />
    );
  }

  return <StorageContent data={state.data} focusCategoryId={focusCategoryId} />;
}
