import { useEffect, useState } from "react";
import {
  isApiError,
  type ApiError,
  type CleanupData,
  type CleanupExecution,
  type CleanupItem,
  type CleanupPreview,
  type CoreSightApi
} from "../api";
import { useApiData } from "../hooks/useApiData";
import {
  formatAge,
  formatBytes,
  formatCount,
  formatDateTime
} from "../lib/format";
import {
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineError,
  InlineNotice,
  PageHeader,
  ScreenError,
  ScreenLoading,
  SectionHeading,
  TierBadge
} from "../components/ui";

type PreviewState =
  | { status: "loading"; item: CleanupItem }
  | { status: "error"; item: CleanupItem; error: ApiError }
  | { status: "ready"; item: CleanupItem; preview: CleanupPreview };

function toActionError(error: unknown): ApiError {
  if (isApiError(error)) return error;
  return {
    code: "UNEXPECTED_UI_ERROR",
    message: "This action could not be completed.",
    detail: "Nothing was moved. Return to the opportunities list and try again."
  };
}

function blockingReason(item: CleanupItem, scanIncomplete: boolean): string | null {
  if (scanIncomplete) {
    return "Cleanup is blocked because the scan is incomplete. A complete scan is required before any file can move.";
  }
  if (!item.previewable) {
    return "This item has no complete preview route and cannot be confirmed.";
  }
  if (item.recoverableBytes === null) {
    return "The recoverable amount is unknown, so CoreSight cannot build a complete preview.";
  }
  return null;
}

function CleanupList({
  data,
  expandedId,
  setExpandedId,
  onPreview
}: {
  data: CleanupData;
  expandedId: string | null;
  setExpandedId: (id: string | null) => void;
  onPreview: (item: CleanupItem) => void;
}) {
  const scanIncomplete = data.coverage.completeness !== "complete";
  const partial = scanIncomplete;
  const actionItems = data.items;
  const offLimits = data.offLimits;

  return (
    <div className="screen screen--cleanup">
      <PageHeader
        eyebrow="Cleanup"
        title="What can I safely remove?"
        description="Ranked opportunities with their reasoning attached. One review, one preview, one explicit confirmation."
        aside={
          <div className="snapshot-stamp">
            <span>Last scan</span>
            <strong>{formatDateTime(data.lastScanAt)}</strong>
            <span className="snapshot-stamp__note">
              {partial
                ? "Partial scan · cleanup actions blocked"
                : "Actions available on this screen only"}
            </span>
          </div>
        }
      />

      <CoverageBanner coverage={data.coverage} />

      <section className="cleanup-summary" aria-labelledby="cleanup-summary-title">
        <div>
          <p className="eyebrow">Safe estimate</p>
          <h2 id="cleanup-summary-title">
            <DataValue partial={partial}>{formatBytes(data.safeRecoverableBytes)}</DataValue>
          </h2>
          <p>Across safe-tier opportunities. Nothing is moved automatically.</p>
        </div>
        <div>
          <p className="eyebrow">Needs review</p>
          <p className="cleanup-summary__figure">
            <DataValue partial={partial}>{formatBytes(data.reviewRecoverableBytes)}</DataValue>
          </p>
          <p>Review-only opportunities, never bundled into a safe action.</p>
        </div>
        <div className="cleanup-summary__rules">
          <strong>No clean-everything path exists.</strong>
          <p>
            Each opportunity has its own preview. Off-limits categories stay visible below with
            the reason they cannot be selected.
          </p>
        </div>
      </section>

      {scanIncomplete ? (
        <InlineNotice tone="attention">
          <strong>Actions are disabled while this scan is incomplete.</strong>
          <p>
            Safe-tier plans require complete knowledge. The opportunities remain visible so you
            can inspect what CoreSight found, but nothing can be previewed for confirmation.
          </p>
        </InlineNotice>
      ) : null}

      <section className="panel" aria-labelledby="opportunities-heading">
        <SectionHeading
          id="opportunities-heading"
          title="Opportunities"
          description="Engine order. Expand one item to read why it is suggested, what stays untouched, and what happens next."
          aside={
            <span className="eyebrow">
              <DataValue partial={partial}>
                {formatCount(actionItems.length, "opportunity", "opportunities")}
              </DataValue>
            </span>
          }
        />

        {actionItems.length === 0 ? (
          <EmptyState
            title="No cleanup opportunities right now"
            description={
              data.lastScanAt === null
                ? "There is no completed scan to produce cleanup opportunities. This is an absence of data, not a claim that nothing can be cleaned."
                : "The last completed scan found nothing eligible for safe or review cleanup. This is an empty result, not a hidden recommendation."
            }
            nextStep={
              data.lastScanAt === null
                ? "Once a complete scan finishes, safe and review opportunities will appear here with their reasoning."
                : "Keep the off-limits summary below for context. New opportunities appear after the next completed scan."
            }
          />
        ) : (
          <ul className="cleanup-list">
            {actionItems.map((item) => {
              const expanded = expandedId === item.id;
              const reason = blockingReason(item, scanIncomplete);
              const blockedId = `cleanup-blocked-${item.id}`;
              return (
                <li className={`cleanup-card cleanup-card--${item.tier.toLowerCase()}`} key={item.id}>
                  <div className="cleanup-card__header">
                    <div className="cleanup-card__title">
                      <TierBadge tier={item.tier} />
                      <h3>{item.title}</h3>
                      <p>{item.whatItIs}</p>
                    </div>
                    <div className="cleanup-card__recoverable">
                      <span>Expected recovery</span>
                      <DataValue partial={partial}>
                        {formatBytes(item.recoverableBytes)}
                      </DataValue>
                    </div>
                    <button
                      className="button button--quiet"
                      type="button"
                      aria-expanded={expanded}
                      aria-controls={`cleanup-detail-${item.id}`}
                      aria-label={`${
                        expanded ? "Hide details for" : "Read details for"
                      } ${item.title}`}
                      onClick={() => setExpandedId(expanded ? null : item.id)}
                    >
                      {expanded ? "Hide details" : "Read the details"}
                    </button>
                  </div>

                  <div className="cleanup-card__quick-facts">
                    <DataValue partial={partial}>
                      {item.estimatedAgeDays === null
                        ? "Age unknown"
                        : `Oldest evidence: ${formatAge(item.estimatedAgeDays)}`}
                    </DataValue>
                    <DataValue partial={partial}>
                      {formatCount(item.itemCount, "entry", "entries")}
                    </DataValue>
                    {item.recoverableBytes === null ? (
                      <span>Recovery amount unknown</span>
                    ) : null}
                  </div>

                  {expanded ? (
                    <div
                      className="cleanup-card__detail"
                      id={`cleanup-detail-${item.id}`}
                    >
                      <div className="explanation-grid">
                        <section>
                          <h4>Why CoreSight suggests this</h4>
                          <p>{item.why}</p>
                        </section>
                        <section>
                          <h4>What stays untouched</h4>
                          <p>{item.whatStaysUntouched}</p>
                        </section>
                        <section>
                          <h4>What happens after removal</h4>
                          <p>{item.consequence}</p>
                        </section>
                      </div>

                      {item.evidence.length > 0 ? (
                        <ul className="evidence-list" aria-label="Evidence">
                          {item.evidence.map((note) => (
                            <li key={`${item.id}-${note.label}`}>
                              <span className="evidence-list__kind">{note.kind}</span>
                              <div>
                                <strong>{note.label}</strong>
                                <p>{note.detail}</p>
                              </div>
                            </li>
                          ))}
                        </ul>
                      ) : null}

                      <div className="cleanup-card__action">
                        <div>
                          <strong>Preview is mandatory.</strong>
                          <p>
                            The next step shows exactly what would move, where it would go, and
                            how to undo it. No file moves until you confirm.
                          </p>
                          {reason ? (
                            <p className="blocked-reason" id={blockedId}>
                              {reason}
                            </p>
                          ) : null}
                        </div>
                        <button
                          className="button button--primary"
                          type="button"
                          disabled={reason !== null}
                          aria-describedby={reason ? blockedId : undefined}
                          onClick={() => onPreview(item)}
                        >
                          Preview what would move
                        </button>
                      </div>
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="panel panel--offlimits" aria-labelledby="offlimits-heading">
        <SectionHeading
          id="offlimits-heading"
          title="Visibly excluded"
          description="Off-limits categories are not hidden. They are shown so the boundary is clear, with no action path."
          aside={
            <span className="eyebrow">
              {formatCount(offLimits.length, "excluded category", "excluded categories")}
            </span>
          }
        />
        {offLimits.length === 0 ? (
          <p className="microcopy">
            {data.lastScanAt === null
              ? "No off-limits categories are available without a completed scan."
              : "No off-limits categories were reported for this drive."}
          </p>
        ) : (
          <ul className="offlimits-list">
            {offLimits.map((item) => (
              <li key={item.id}>
                <div className="offlimits-list__title">
                  <TierBadge tier={item.tier} />
                  <h3>{item.title}</h3>
                </div>
                <p>{item.whatItIs}</p>
                <p className="offlimits-list__reason">
                  <strong>Why it is excluded:</strong>{" "}
                  {item.offLimitsReason ?? "Protected by engine policy."}
                </p>
                <p className="microcopy">
                  {item.observedBytes === null
                    ? "Size unknown — this category is never counted as reclaimable."
                    : (
                      <DataValue partial={partial}>
                        {formatBytes(item.observedBytes)} observed. This is context, not
                        recoverable space.
                      </DataValue>
                    )}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function PreviewView({
  item,
  previewState,
  acknowledged,
  setAcknowledged,
  executing,
  actionError,
  onBack,
  onConfirm
}: {
  item: CleanupItem;
  previewState: PreviewState;
  acknowledged: boolean;
  setAcknowledged: (value: boolean) => void;
  executing: boolean;
  actionError: ApiError | null;
  onBack: () => void;
  onConfirm: (preview: CleanupPreview) => void;
}) {
  const preview = previewState.status === "ready" ? previewState.preview : null;
  const scanIncomplete = preview?.coverage.completeness !== "complete";
  const confirmBlocked =
    preview === null ||
    !preview.complete ||
    scanIncomplete ||
    preview.totalBytes === null ||
    !acknowledged ||
    executing;

  return (
    <div className="screen screen--cleanup">
      <PageHeader
        eyebrow="Cleanup · Preview"
        title={`Review: ${item.title}`}
        description="Step two of two. This is the exact statement you confirm; nothing has moved yet."
      />
      <button className="back-link" type="button" onClick={onBack}>
        <span aria-hidden="true">←</span> Back to opportunities
      </button>

      {previewState.status === "loading" ? (
        <section className="state-panel state-panel--loading" role="status" aria-live="polite">
          <div className="state-panel__heading">
            <span className="spinner" aria-hidden="true" />
            <h2>Building the exact preview</h2>
          </div>
          <p>Checking the selected items, destination, undo route, and byte accounting.</p>
        </section>
      ) : null}

      {previewState.status === "error" ? <InlineError error={previewState.error} /> : null}

      {preview ? (
        <>
          <CoverageBanner coverage={preview.coverage} context="cleanup preview" />
          <article className="preview-sheet" aria-labelledby="preview-heading">
            <header className="preview-sheet__header">
              <p className="eyebrow">Exact preview · nothing has moved</p>
              <h2 id="preview-heading">{item.title}</h2>
              <p>{preview.consequence}</p>
              <div className="preview-sheet__total">
                <span>Would be moved</span>
                <DataValue partial={preview.coverage.completeness !== "complete"}>
                  {formatBytes(preview.totalBytes)}
                </DataValue>
                <span>{formatCount(preview.items.length, "preview line", "preview lines")}</span>
              </div>
            </header>

            <div className="preview-columns">
              <section aria-labelledby="what-moves-heading">
                <h3 id="what-moves-heading">Exactly what would move</h3>
                <ul className="preview-items">
                  {preview.items.map((previewItem) => (
                    <li key={previewItem.title}>
                      <div>
                        <strong>{previewItem.title}</strong>
                        <p>{previewItem.reason}</p>
                      </div>
                      <div className="preview-items__numbers">
                        <DataValue partial={preview.coverage.completeness !== "complete"}>
                          {formatBytes(previewItem.bytes)}
                        </DataValue>
                        <span>
                          {formatCount(previewItem.itemCount, "entry", "entries")}
                        </span>
                      </div>
                    </li>
                  ))}
                </ul>
              </section>

              <section aria-labelledby="where-heading">
                <h3 id="where-heading">Where it goes</h3>
                <p className="preview-route">
                  {preview.destination ?? "Destination unknown — confirmation is blocked."}
                </p>
                <h3>How to undo</h3>
                <p>{preview.howToUndo ?? "Undo route unknown — confirmation is blocked."}</p>
                <h3>What stays untouched</h3>
                <ul className="stays-list">
                  {preview.whatStaysUntouched.map((statement) => (
                    <li key={statement}>{statement}</li>
                  ))}
                </ul>
              </section>
            </div>

            {preview.warnings.length > 0 ? (
              <div className="preview-warnings" role="note">
                <strong>Warnings to read before confirming</strong>
                <ul>
                  {preview.warnings.map((warning) => (
                    <li key={warning}>{warning}</li>
                  ))}
                </ul>
              </div>
            ) : null}

            {!preview.complete || scanIncomplete ? (
              <InlineNotice tone="attention">
                <strong>This preview cannot be confirmed.</strong>
                <p>
                  A destination, undo route, byte amount, or complete scan is missing. CoreSight
                  blocks the plan rather than offering a partial confirmation.
                </p>
              </InlineNotice>
            ) : null}

            {actionError ? <InlineError error={actionError} /> : null}

            <div className="confirm-zone">
              <label className="confirm-check">
                <input
                  type="checkbox"
                  checked={acknowledged}
                  aria-label="I have read exactly what would move, where it would go, and how to undo it."
                  onChange={(event) => setAcknowledged(event.target.checked)}
                  disabled={!preview.complete || scanIncomplete || executing}
                />
                <span>
                  I have read exactly what would move, where it would go, and how to undo it. I
                  understand that Trash only protects these items until Trash is emptied.
                </span>
              </label>
              <button
                className="button button--primary"
                type="button"
                disabled={confirmBlocked}
                onClick={() => onConfirm(preview)}
              >
                {executing ? "Confirming…" : "Confirm move to Trash"}
              </button>
              <p className="microcopy">
                One opportunity at a time. No action is taken for any other cleanup item.
              </p>
            </div>
            <div className="visually-hidden" role="status" aria-live="polite">
              {executing ? "Confirming cleanup" : acknowledged ? "Confirmation checked" : ""}
            </div>
          </article>
        </>
      ) : null}
    </div>
  );
}

function ResultView({
  execution,
  itemTitle,
  onReturn
}: {
  execution: CleanupExecution;
  itemTitle: string;
  onReturn: () => void;
}) {
  return (
    <div className="screen screen--cleanup">
      <PageHeader
        eyebrow="Cleanup · Confirmation"
        title="Confirmation recorded"
        description={`The cleanup for “${itemTitle}” passed through the preview gate.`}
      />
      <section className="result-panel" aria-labelledby="result-heading">
        <span className="result-panel__mark" aria-hidden="true">
          ✓
        </span>
        <p className="eyebrow">Result</p>
        <h2 id="result-heading">
          {execution.simulated
            ? "UI preview completed in memory"
            : "Cleanup request accepted"}
        </h2>
        <p>{execution.note}</p>
        <dl className="result-facts">
          <div>
            <dt>Freed bytes</dt>
            <dd>
              {execution.freedBytes === null
                ? "Unknown"
                : formatBytes(execution.freedBytes)}
            </dd>
          </div>
          <div>
            <dt>Entries confirmed</dt>
            <dd>
              {execution.movedItems === null
                ? "Unknown"
                : formatCount(execution.movedItems, "entry", "entries")}
            </dd>
          </div>
          <div>
            <dt>Verification</dt>
            <dd>{execution.verification}</dd>
          </div>
        </dl>
        {execution.leftovers.length > 0 ? (
          <div className="inline-message inline-message--attention">
            <strong>Leftovers were reported</strong>
            <ul>
              {execution.leftovers.map((leftover) => (
                <li key={leftover}>{leftover}</li>
              ))}
            </ul>
          </div>
        ) : null}
        <p className="microcopy">
          No permanent deletion is offered. In this Phase 8 shell the mock adapter does not touch
          the filesystem; Phase 9 wires the authorized IPC action.
        </p>
        <button className="button button--primary" type="button" onClick={onReturn}>
          Review remaining opportunities
        </button>
      </section>
    </div>
  );
}

function CleanupContent({
  api,
  data,
  onRefresh
}: {
  api: CoreSightApi;
  data: CleanupData;
  onRefresh: () => void;
}) {
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [previewState, setPreviewState] = useState<PreviewState | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [executing, setExecuting] = useState(false);
  const [execution, setExecution] = useState<CleanupExecution | null>(null);
  const [resultTitle, setResultTitle] = useState<string>("selected cleanup");
  const [actionError, setActionError] = useState<ApiError | null>(null);
  const viewMode = execution !== null ? "result" : previewState !== null ? "preview" : "list";

  useEffect(() => {
    if (viewMode === "list") return;
    window.scrollTo({ top: 0, behavior: "auto" });
    document.querySelector<HTMLElement>("#content h1")?.focus({ preventScroll: true });
  }, [viewMode]);

  async function handlePreview(item: CleanupItem) {
    setActionError(null);
    setAcknowledged(false);
    setPreviewState({ status: "loading", item });
    try {
      const preview = await api.previewCleanup([item.id]);
      setPreviewState({ status: "ready", item, preview });
    } catch (error: unknown) {
      setPreviewState({ status: "error", item, error: toActionError(error) });
    }
  }

  async function handleConfirm(preview: CleanupPreview) {
    if (!acknowledged || !preview.complete) return;
    setExecuting(true);
    setActionError(null);
    try {
      const result = await api.executeCleanup({
        planId: preview.planId,
        confirmed: true,
        acknowledgedWarnings: preview.warnings
      });
      setResultTitle(previewState?.status === "ready" ? previewState.item.title : "selected cleanup");
      setExecution(result);
      setPreviewState(null);
    } catch (error: unknown) {
      setActionError(toActionError(error));
    } finally {
      setExecuting(false);
    }
  }

  if (execution !== null) {
    return (
      <ResultView
        execution={execution}
        itemTitle={resultTitle}
        onReturn={() => {
          setExecution(null);
          onRefresh();
        }}
      />
    );
  }

  if (previewState !== null) {
    return (
      <PreviewView
        item={previewState.item}
        previewState={previewState}
        acknowledged={acknowledged}
        setAcknowledged={setAcknowledged}
        executing={executing}
        actionError={actionError}
        onBack={() => {
          setPreviewState(null);
          setAcknowledged(false);
          setActionError(null);
        }}
        onConfirm={handleConfirm}
      />
    );
  }

  return (
    <CleanupList
      data={data}
      expandedId={expandedId}
      setExpandedId={setExpandedId}
      onPreview={handlePreview}
    />
  );
}

export function CleanupScreen({ api }: { api: CoreSightApi }) {
  const [refreshKey, setRefreshKey] = useState(0);
  const state = useApiData(() => api.getCleanup(), [api, refreshKey]);

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow="Cleanup"
        title="What can I safely remove?"
        description="Ranked opportunities with their reasoning attached. One review, one preview, one explicit confirmation."
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow="Cleanup"
        title="What can I safely remove?"
        description="Ranked opportunities with their reasoning attached. One review, one preview, one explicit confirmation."
        error={state.error}
      />
    );
  }

  return (
    <CleanupContent
      api={api}
      data={state.data}
      onRefresh={() => setRefreshKey((key) => key + 1)}
    />
  );
}
