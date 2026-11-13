import { useEffect, useRef, useState } from "react";
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
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  Field,
  KeyHint,
  PageHeader,
  Panel,
  ScreenError,
  ScreenLoading,
  ShareBar,
  StatusPill,
  Surface,
  UnknownValue
} from "../components/ui";
import { Treemap } from "../components/viz";

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
  return category.id === id || category.children.some((child) => containsCategory(child, id));
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
  const shouldOpen =
    forceOpen || (focusCategoryId !== null && containsCategory(category, focusCategoryId));
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

  /**
   * A category's children are mounted only while it is open.
   *
   * The engine's category tree is recursive and uncapped, so mounting every
   * descendant eagerly puts the whole tree into the DOM the moment the screen
   * opens — for a large volume that is tens of thousands of nodes the reader
   * cannot see and did not ask for. A closed branch costs nothing to show, so
   * it must cost nothing to mount.
   *
   * The native `<details>` element remains the control: it is keyboard
   * operable, it is announced correctly, and it keeps its open state without
   * a button (this screen may not carry one).
   */
  const [expanded, setExpanded] = useState(shouldOpen);
  const detailsRef = useRef<HTMLDetailsElement>(null);

  useEffect(() => {
    const node = detailsRef.current;
    if (node === null) return;
    const sync = () => setExpanded(node.open);
    sync();
    // React 18 does not expose an `onToggle` prop for <details>, so the
    // listener is attached directly. The `toggle` event fires after the
    // browser has updated `open`, so the React state and the DOM never
    // disagree about what is open.
    node.addEventListener("toggle", sync);
    return () => node.removeEventListener("toggle", sync);
  }, []);

  return (
    <details
      ref={detailsRef}
      className={`storage-node storage-node--depth-${Math.min(depth, 2)}${
        isFocused ? " storage-node--focused" : ""
      }`}
      open={shouldOpen || undefined}
    >
      <summary className="storage-row" aria-label={summaryLabel}>
        <span className="storage-row__category">
          <span className="storage-row__chevron" aria-hidden="true" />
          <span className="storage-row__name">
            <span className="storage-row__title">{category.name}</span>
            <span className="storage-row__meta">{childrenLabel}</span>
          </span>
        </span>
        <span className="storage-row__share">
          <ShareBar
            share={category.shareOfParent}
            label={t("storage.category.shareLabel", { name: category.name })}
            partial={partial}
          />
          <span className="cs-num">
            {category.shareOfParent === null ? (
              <UnknownValue />
            ) : (
              formatPercent(category.shareOfParent)
            )}
          </span>
        </span>
        <span className="storage-row__bytes">
          <Bytes value={category.bytes} partial={partial} />
        </span>
        <span className="storage-row__delta cs-num">
          {formatDelta(category.deltaSinceLast)}
        </span>
      </summary>

      <div className="storage-detail">
        <dl className="storage-facts">
          <div>
            <dt>{t("storage.category.entries")}</dt>
            <dd>
              {category.itemCount === null ? (
                <UnknownValue />
              ) : (
                <DataValue partial={partial}>{entryCount(category.itemCount)}</DataValue>
              )}
            </dd>
          </div>
          <div>
            <dt>{t("storage.category.entriesWithoutSize")}</dt>
            <dd>
              {category.entriesWithoutSize === null ? (
                <UnknownValue />
              ) : (
                <DataValue partial={partial}>{entryCount(category.entriesWithoutSize)}</DataValue>
              )}
            </dd>
          </div>
          <div>
            <dt>{t("storage.category.observationErrors")}</dt>
            <dd>
              {category.observationErrors === null ? (
                <UnknownValue />
              ) : (
                <DataValue partial={partial}>{entryCount(category.observationErrors)}</DataValue>
              )}
            </dd>
          </div>
        </dl>

        {category.children.length === 0 ? (
          <p className="cs-micro">{t("storage.category.noDeeperBody")}</p>
        ) : expanded ? (
          <ul className="storage-children">
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
        ) : null}

        {category.sampleLocations.length > 0 ? (
          <details className="cs-disclosure location-disclosure">
            <summary
              aria-label={t("storage.category.sampleLocationsAria", { name: category.name })}
            >
              <span>{t("storage.category.sampleLocations")}</span>
              <span className="cs-disclosure__count">
                {tCount(category.sampleLocations.length, "unit.label.one", "unit.label.other")}
              </span>
            </summary>
            <p className="cs-micro">{t("storage.category.sampleLocationsNote")}</p>
            <dl className="location-list">
              {category.sampleLocations.map((location) => (
                <div key={`${category.id}-${location.displayPath}`}>
                  <dt>{location.label}</dt>
                  <dd>
                    <code>{location.displayPath}</code>
                    <Bytes value={location.bytes} partial={partial} />
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
  const searchRef = useRef<HTMLInputElement>(null);
  const normalisedQuery = query.trim().toLocaleLowerCase();
  const partial = data.coverage.completeness !== "complete";

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target;
      const typing =
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLSelectElement;
      if (event.key === "/" && !typing) {
        event.preventDefault();
        searchRef.current?.focus();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (data.categories.length === 0) {
    return (
      <div className="screen">
        <PageHeader
          eyebrow={t("storage.eyebrow")}
          title={t("storage.title")}
          description={t("storage.description")}
        />
        <CoverageBanner complete={!partial} coverage={data.coverage} context="storageScan" />
        <EmptyState
          title={t("storage.empty.title")}
          body={t("storage.empty.body")}
          next={t("storage.empty.next")}
        />
      </div>
    );
  }

  const filteredCategories = data.categories
    .map((category) => filterCategory(category, normalisedQuery))
    .filter((category): category is CategoryTotals => category !== null);
  const matchingTopLevel =
    normalisedQuery.length > 0 ? filteredCategories.length : data.categories.length;

  return (
    <div className="screen screen--storage">
      <PageHeader
        eyebrow={t("storage.eyebrow")}
        title={t("storage.title")}
        description={t("storage.descriptionMain")}
      />

      <CoverageBanner complete={!partial} coverage={data.coverage} context="storageScan" />

      <Surface tone="raised" padded className="storage-summary">
        <div className="storage-summary__top">
          <div className="storage-summary__lead">
            <p className="cs-eyebrow">{t("storage.summary.observedTotal")}</p>
            <p className="storage-summary__figure cs-num">
              <Bytes value={data.totalObservedBytes} partial={partial} />
            </p>
            <p className="storage-summary__body">
              {t(partial ? "storage.summary.bodyPartial" : "storage.summary.bodyComplete", {
                count: observedEntryCount(data.totalObservedItems)
              })}
            </p>
            <div className="storage-summary__status">
              <StatusPill tone={partial ? "attention" : "positive"}>
                {t(partial ? "status.partial" : "status.complete")}
              </StatusPill>
              <span className="cs-micro">{t("home.stamp.localOnly")}</span>
            </div>
          </div>
          <dl className="storage-summary__facts">
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
                {data.coverage.unknownSizeEntries === null ? (
                  <UnknownValue />
                ) : (
                  <DataValue partial={partial}>
                    {entryCount(data.coverage.unknownSizeEntries)}
                  </DataValue>
                )}
              </dd>
            </div>
            <div>
              <dt>{t("storage.summary.duplicateEvidence")}</dt>
              <dd>
                <DataValue
                  partial={data.duplicateReport.coverage.completeness !== "complete"}
                >
                  {t(
                    data.duplicateReport.coverage.completeness === "complete"
                      ? "status.complete"
                      : "status.partial"
                  )}
                </DataValue>
              </dd>
            </div>
          </dl>
        </div>
        <Treemap
          nodes={data.categories.map((category) => ({
            id: category.id,
            label: category.name,
            bytes: category.bytes
          }))}
          height={220}
        />
      </Surface>

      <Panel
        id="storage-categories-heading"
        title={t("storage.categories.title")}
        description={t("storage.categories.description")}
        actions={
          <span className="cs-eyebrow">
            <DataValue partial={partial}>
              {tCount(matchingTopLevel, "unit.topLevelGroup.one", "unit.topLevelGroup.other")}
            </DataValue>
          </span>
        }
        className="storage-categories"
      >
        <div className="storage-toolbar">
          <Field
            label={t("storage.search.label")}
            hint={
              normalisedQuery.length === 0
                ? t("storage.search.help")
                : t("storage.search.status", {
                    matches: tCount(
                      filteredCategories.length,
                      "unit.match.one",
                      "unit.match.other"
                    ),
                    query: query.trim()
                  })
            }
            hintId="storage-search-help"
            htmlFor="storage-search"
            className="search-field"
          >
            <input
              id="storage-search"
              ref={searchRef}
              type="search"
              value={query}
              aria-label={t("storage.search.aria")}
              aria-describedby="storage-search-help"
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") setQuery("");
              }}
              placeholder={t("storage.search.placeholder")}
              autoComplete="off"
            />
          </Field>
          <span className="storage-toolbar__keys">
            <KeyHint keys={["/"]} />
          </span>
        </div>
        <p className="visually-hidden" aria-live="polite">
          {normalisedQuery.length === 0
            ? ""
            : t("storage.search.status", {
                matches: tCount(
                  filteredCategories.length,
                  "unit.match.one",
                  "unit.match.other"
                ),
                query: query.trim()
              })}
        </p>

        {filteredCategories.length === 0 ? (
          <EmptyState
            title={t("storage.search.emptyTitle")}
            body={t("storage.search.emptyBody", { query: query.trim() })}
            next={t("storage.search.emptyNext")}
          />
        ) : (
          <>
            <div className="storage-table__head" aria-hidden="true">
              <span>{t("storage.column.category")}</span>
              <span>{t("storage.column.shareOfTotal")}</span>
              <span>{t("storage.column.observed")}</span>
              <span>{t("storage.column.change")}</span>
            </div>
            <ul className="storage-list">
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
      </Panel>

      <div className="evidence-grid">
        <Panel
          id="duplicate-heading"
          title={t("storage.duplicates.title")}
          description={t("storage.duplicates.description")}
          className="evidence-panel"
        >
          <CoverageBanner
            complete={data.duplicateReport.coverage.completeness === "complete"}
            coverage={data.duplicateReport.coverage}
            context="duplicateAnalysis"
          />
          <div className="evidence-summary">
            <span>{t("storage.duplicates.recoverable")}</span>
            <span className="evidence-summary__value">
              <Bytes
                value={data.duplicateReport.totalRecoverableBytes}
                partial={data.duplicateReport.coverage.completeness !== "complete"}
              />
            </span>
          </div>
          {data.duplicateReport.groups.length === 0 ? (
            <p className="cs-micro">{t("storage.duplicates.empty")}</p>
          ) : (
            <div className="duplicate-list">
              {data.duplicateReport.groups.map((group, index) => (
                <article className="duplicate-group" key={group.id}>
                  <header>
                    <div>
                      <p className="cs-eyebrow">
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
                  <details className="cs-disclosure member-disclosure">
                    <summary
                      aria-label={t("storage.duplicates.showMembersAria", {
                        index: index + 1
                      })}
                    >
                      <span>{t("storage.duplicates.showMembers")}</span>
                      <span className="cs-disclosure__count">
                        {tCount(group.members.length, "unit.member.one", "unit.member.other")}
                      </span>
                    </summary>
                    <ul className="duplicate-members">
                      {group.members.map((member) => (
                        <li key={`${group.id}-${member.displayPath}`}>
                          <code>{member.displayPath}</code>
                          <span>
                            {member.sizeBytes === null ? (
                              <UnknownValue />
                            ) : (
                              formatBytes(member.sizeBytes)
                            )}
                            {member.isHardlinkAlias
                              ? t("storage.duplicates.hardlinkAlias")
                              : ""}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </details>
                  {group.members.some((member) => member.isHardlinkAlias) ? (
                    <p className="cs-micro">{t("storage.duplicates.hardlinkNote")}</p>
                  ) : null}
                </article>
              ))}
            </div>
          )}
          {data.duplicateReport.unverifiedEntries !== null &&
          data.duplicateReport.unverifiedEntries > 0 ? (
            <p className="cs-micro">
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
            <p className="cs-micro">
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
        </Panel>

        <Panel
          id="big-files-heading"
          title={t("storage.bigFiles.title")}
          description={t("storage.bigFiles.description", {
            bytes: formatBytes(data.bigFileReport.thresholdBytes)
          })}
          className="evidence-panel"
        >
          <CoverageBanner
            complete={data.bigFileReport.coverage.completeness === "complete"}
            coverage={data.bigFileReport.coverage}
            context="largeFileResult"
          />
          {data.bigFileReport.files.length === 0 ? (
            <p className="cs-micro">{t("storage.bigFiles.empty")}</p>
          ) : (
            <ul className="bigfile-list">
              {data.bigFileReport.files.map((file) => (
                <li key={file.id}>
                  <div className="bigfile__headline">
                    <span className="bigfile__size">
                      <Bytes
                        value={file.sizeBytes}
                        partial={data.bigFileReport.coverage.completeness !== "complete"}
                      />
                    </span>
                    <span className="bigfile__category">
                      {file.categoryName ?? t("storage.bigFiles.categoryUnknown")}
                    </span>
                  </div>
                  <div className="bigfile__meta">
                    <span>
                      {file.ageDays === null ? (
                        <UnknownValue label={t("format.ageUnknown")} />
                      ) : (
                        t("storage.bigFiles.age", { age: formatAge(file.ageDays) })
                      )}
                    </span>
                    <span>
                      {file.modifiedAt === null ? (
                        <UnknownValue label={t("storage.bigFiles.modifiedUnknown")} />
                      ) : (
                        t("storage.bigFiles.modified", {
                          time: formatDateTime(file.modifiedAt)
                        })
                      )}
                    </span>
                  </div>
                  <details className="cs-disclosure file-location">
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
            <p className="cs-micro">
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
        </Panel>
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
        layout="table"
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
