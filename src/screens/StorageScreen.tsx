import { useState } from "react";
import type { CategoryTotals, CoreSightApi, StorageData } from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatAge,
  formatBytes,
  formatCount,
  formatDateTime,
  formatDelta,
  formatPercent
} from "../lib/format";
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
  const summaryLabel = `${category.name}: ${formatBytes(category.bytes)} observed, ${formatPercent(
    category.shareOfParent
  )} of parent, ${formatDelta(category.deltaSinceLast)} since the last scan${
    partial ? ", from a partial source" : ""
  }. ${
    knownChildren > 0
      ? formatCount(knownChildren, "subcategory", "subcategories")
      : "No deeper breakdown"
  }.`;

  return (
    <details
      className={`category category--depth-${Math.min(depth, 2)}${isFocused ? " category--focused" : ""}`}
      open={shouldOpen || undefined}
    >
      <summary className="category-summary" aria-label={summaryLabel}>
        <span className="category-summary__chevron" aria-hidden="true" />
        <span className="category-summary__name">
          <span className="category-summary__title">{category.name}</span>
          <span className="category-summary__meta">
            {knownChildren > 0
              ? formatCount(knownChildren, "subcategory", "subcategories")
              : "No deeper breakdown"}
          </span>
        </span>
        <SizeBar
          share={category.shareOfParent}
          label={`${category.name} share`}
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
            <dt>Entries</dt>
            <dd>
              <DataValue partial={partial}>
                {formatCount(category.itemCount, "entry", "entries")}
              </DataValue>
            </dd>
          </div>
          <div>
            <dt>Entries without a known size</dt>
            <dd>
              <DataValue partial={partial}>
                {category.entriesWithoutSize === null
                  ? "Unknown"
                  : formatCount(category.entriesWithoutSize, "entry", "entries")}
              </DataValue>
            </dd>
          </div>
          <div>
            <dt>Observation errors</dt>
            <dd>
              <DataValue partial={partial}>
                {category.observationErrors === null
                  ? "Unknown"
                  : formatCount(category.observationErrors, "entry", "entries")}
              </DataValue>
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
          <p className="microcopy">No deeper category breakdown was recorded.</p>
        )}

        {category.sampleLocations.length > 0 ? (
          <details className="location-disclosure">
            <summary aria-label={`Sample locations for ${category.name}`}>
              <span>Sample locations</span>
              <span className="location-disclosure__count">
                {formatCount(category.sampleLocations.length, "label", "labels")}
              </span>
            </summary>
            <p className="microcopy">
              Labels are supplied by the engine and displayed verbatim. CoreSight does not build
              or resolve paths in the interface.
            </p>
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
          eyebrow="Storage"
          title="What's using my space?"
          description="Human categories first, technical labels one deliberate step down."
        />
        <CoverageBanner coverage={data.coverage} />
        <EmptyState
          title="No storage categories yet"
          description="There is no completed category breakdown for this device. Zero categories is a real empty result, not an unknown total."
          nextStep="After a scan completes, categories, verified duplicates, and large files will appear here."
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
        eyebrow="Storage"
        title="What's using my space?"
        description="Start with human categories. Expand only when you want the evidence underneath."
      />

      <CoverageBanner coverage={data.coverage} />

      <section className="storage-summary" aria-labelledby="storage-summary-title">
        <div className="storage-summary__lead">
          <p className="eyebrow">Observed total</p>
          <h2 id="storage-summary-title">
            <DataValue partial={partial}>{formatBytes(data.totalObservedBytes)}</DataValue>
          </h2>
          <p>
            Across{" "}
            <DataValue partial={partial}>
              {formatCount(data.totalObservedItems, "observed entry", "observed entries")}
            </DataValue>
            . This is not free space; it is what the {partial ? "partial " : ""}scan could
            observe.
          </p>
        </div>
        <dl className="summary-facts">
          <div>
            <dt>Categories</dt>
            <dd>
              <DataValue partial={partial}>
                {formatCount(data.categories.length, "category", "categories")}
              </DataValue>
            </dd>
          </div>
          <div>
            <dt>Entries without a size</dt>
            <dd>
              {data.coverage.unknownSizeEntries === null
                ? "Unknown"
                : formatCount(data.coverage.unknownSizeEntries, "entry", "entries")}
              {partial ? <PartialTag /> : null}
            </dd>
          </div>
          <div>
            <dt>Duplicate evidence</dt>
            <dd>
              {data.duplicateReport.coverage.completeness === "complete"
                ? "Complete"
                : "Partial"}
            </dd>
          </div>
        </dl>
      </section>

      <section className="panel storage-categories" aria-labelledby="storage-categories-heading">
        <SectionHeading
          id="storage-categories-heading"
          title="Categories"
          description="Sorted by observed size. Raw locations live only inside the deepest disclosure."
          aside={
            <span className="eyebrow">
              <DataValue partial={partial}>
                {formatCount(matchingTopLevel, "top-level group", "top-level groups")}
              </DataValue>
            </span>
          }
        />

        <div className="storage-toolbar">
          <label className="search-field" htmlFor="storage-search">
            <span>Search categories</span>
            <input
              id="storage-search"
              type="search"
              value={query}
              aria-label="Search storage categories"
              aria-describedby="storage-search-help"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Games, Documents, Development…"
              autoComplete="off"
            />
          </label>
          <p className="storage-toolbar__status" id="storage-search-help" aria-live="polite">
            {normalisedQuery.length === 0
              ? "Search by human category name, not by path."
              : `${formatCount(filteredCategories.length, "group", "groups")} match “${query.trim()}”.`}
          </p>
        </div>

        {filteredCategories.length === 0 ? (
          <EmptyState
            title="No matching categories"
            description={`No human category contains “${query.trim()}”. This search does not guess at path fragments.`}
            nextStep="Try a broader category name, or clear the search to see every category."
          />
        ) : (
          <>
            <div className="category-columns" aria-hidden="true">
              <span />
              <span>Category</span>
              <span>Share of total</span>
              <span>Observed</span>
              <span>Share</span>
              <span>Change</span>
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
            title="Verified duplicate groups"
            description="Only SHA-256 matches with equal sizes are called duplicates. Hard-link aliases are shown, never counted as savings."
          />
          <CoverageBanner
            coverage={data.duplicateReport.coverage}
            context="duplicate analysis"
          />
          <div className="evidence-panel__summary">
            <span>Recoverable, verified</span>
            <DataValue partial={data.duplicateReport.coverage.completeness !== "complete"}>
              {formatBytes(data.duplicateReport.totalRecoverableBytes)}
            </DataValue>
          </div>
          {data.duplicateReport.groups.length === 0 ? (
            <p className="microcopy">No hash-verified duplicate groups were found.</p>
          ) : (
            <div className="duplicate-list">
              {data.duplicateReport.groups.map((group, index) => (
                <article className="duplicate-group" key={group.id}>
                  <header>
                    <div>
                      <p className="eyebrow">Group {String(index + 1).padStart(2, "0")}</p>
                      <h3>
                        {formatCount(group.members.length, "verified copy", "verified copies")} at{" "}
                        {formatBytes(group.sizeBytes)} each
                      </h3>
                      <p className="duplicate-group__recoverable">
                        Recoverable{" "}
                        <DataValue
                          partial={data.duplicateReport.coverage.completeness !== "complete"}
                        >
                          {formatBytes(group.recoverableBytes)}
                        </DataValue>
                      </p>
                    </div>
                    <StatusPill tone="positive">SHA-256 verified</StatusPill>
                  </header>
                  <p className="hash-line">
                    Hash <code>{group.contentSha256.slice(0, 12)}…</code>
                  </p>
                  <details className="member-disclosure">
                    <summary aria-label={`Show member labels for duplicate group ${index + 1}`}>
                      <span>Show member labels</span>
                      <span>
                        {formatCount(group.members.length, "member", "members")}
                      </span>
                    </summary>
                    <ul className="duplicate-members">
                      {group.members.map((member) => (
                        <li key={`${group.id}-${member.displayPath}`}>
                          <code>{member.displayPath}</code>
                          <span>
                            {formatBytes(member.sizeBytes)}
                            {member.isHardlinkAlias ? " · hard-link alias" : ""}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </details>
                  {group.members.some((member) => member.isHardlinkAlias) ? (
                    <p className="microcopy">
                      A hard-link alias is another name for the same filesystem object. Removing
                      it frees no space, so it contributes nothing to recoverable bytes.
                    </p>
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
                {formatCount(
                  data.duplicateReport.unverifiedEntries,
                  "file lacks",
                  "files lack"
                )}
              </DataValue>{" "}
              a verified content hash and is excluded from every duplicate claim.
            </p>
          ) : null}
          {data.duplicateReport.unknownSizeEntries !== null &&
          data.duplicateReport.unknownSizeEntries > 0 ? (
            <p className="microcopy">
              <DataValue
                partial={data.duplicateReport.coverage.completeness !== "complete"}
              >
                {formatCount(
                  data.duplicateReport.unknownSizeEntries,
                  "entry has",
                  "entries have"
                )}
              </DataValue>{" "}
              a verified hash but no observed size; size equality cannot be proven, so they are
              never grouped.
            </p>
          ) : null}
        </section>

        <section className="panel evidence-panel" aria-labelledby="big-files-heading">
          <SectionHeading
            id="big-files-heading"
            title="Large files"
            description={`Files above ${formatBytes(data.bigFileReport.thresholdBytes)}. Unknown age is never treated as old.`}
          />
          <CoverageBanner coverage={data.bigFileReport.coverage} context="large-file result" />
          {data.bigFileReport.files.length === 0 ? (
            <p className="microcopy">No files crossed the current size threshold.</p>
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
                    <span>{file.categoryName ?? "Category unknown"}</span>
                  </div>
                  <div className="big-file-list__meta">
                    <span>
                      {file.ageDays === null
                        ? "Age unknown"
                        : `${formatAge(file.ageDays)} old`}
                    </span>
                    <span>
                      {file.modifiedAt === null
                        ? "Modified time unknown"
                        : `Modified ${formatDateTime(file.modifiedAt)}`}
                    </span>
                  </div>
                  <details className="file-location">
                    <summary aria-label={`Show location label for the ${formatBytes(file.sizeBytes)} file`}>
                      Show location label
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
                {formatCount(
                  data.bigFileReport.unknownAgeEntries,
                  "entry had",
                  "entries had"
                )}
              </DataValue>{" "}
              an unknown modification time and was excluded from age-based conclusions.
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
        eyebrow="Storage"
        title="What's using my space?"
        description="Human categories first, technical labels one deliberate step down."
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow="Storage"
        title="What's using my space?"
        description="Human categories first, technical labels one deliberate step down."
        error={state.error}
      />
    );
  }

  return <StorageContent data={state.data} focusCategoryId={focusCategoryId} />;
}
