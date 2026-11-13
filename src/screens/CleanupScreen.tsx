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
import { formatAge, formatBytes, formatDateTime } from "../lib/format";
import { t, tCount } from "../i18n";
import {
  Button,
  Bytes,
  CoverageBanner,
  DataValue,
  EmptyState,
  InlineError,
  InlineNotice,
  PageHeader,
  Panel,
  ScreenError,
  ScreenLoading,
  Surface,
  Tier,
  UnknownValue
} from "../components/ui";

type PreviewState =
  | { status: "loading"; item: CleanupItem }
  | { status: "error"; item: CleanupItem; error: ApiError }
  | { status: "ready"; item: CleanupItem; preview: CleanupPreview };

function toActionError(error: unknown): ApiError {
  if (isApiError(error)) return error;
  return {
    code: "UNEXPECTED_UI_ERROR",
    message: t("cleanup.error.message"),
    detail: t("cleanup.error.detail")
  };
}

function entryCount(value: number | null): string {
  if (value === null) return t("state.unknown");
  return tCount(value, "unit.entry.one", "unit.entry.other");
}

function blockingReason(item: CleanupItem, scanIncomplete: boolean): string | null {
  if (scanIncomplete) return t("cleanup.blocking.scanIncomplete");
  if (!item.previewable) return t("cleanup.blocking.noPreview");
  if (item.recoverableBytes === null) return t("cleanup.blocking.unknownBytes");
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

  return (
    <div className="screen screen--cleanup">
      <PageHeader
        eyebrow={t("cleanup.eyebrow")}
        title={t("cleanup.title")}
        description={t("cleanup.description")}
        aside={
          <div className="snapshot-stamp">
            <span>{t("cleanup.stamp.lastScan")}</span>
            <strong>{formatDateTime(data.lastScanAt)}</strong>
            <span className="snapshot-stamp__note">
              {partial ? t("cleanup.stamp.partial") : t("cleanup.stamp.actionsHere")}
            </span>
          </div>
        }
      />

      <CoverageBanner complete={!partial} coverage={data.coverage} context="scan" />

      <Surface tone="raised" padded className="cleanup-summary">
        <div className="cleanup-summary__cell cleanup-summary__cell--safe">
          <p className="cs-eyebrow">{t("cleanup.summary.safeEyebrow")}</p>
          <p className="cleanup-summary__figure cs-num">
            <Bytes value={data.safeRecoverableBytes} partial={partial} />
          </p>
          <p className="cleanup-summary__body">{t("cleanup.summary.safeBody")}</p>
        </div>
        <div className="cleanup-summary__cell">
          <p className="cs-eyebrow">{t("cleanup.summary.reviewEyebrow")}</p>
          <p className="cleanup-summary__figure cleanup-summary__figure--small cs-num">
            <Bytes value={data.reviewRecoverableBytes} partial={partial} />
          </p>
          <p className="cleanup-summary__body">{t("cleanup.summary.reviewBody")}</p>
        </div>
        <div className="cleanup-summary__cell cleanup-summary__rules">
          <strong>{t("cleanup.rules.title")}</strong>
          <p>{t("cleanup.rules.body")}</p>
        </div>
      </Surface>

      {scanIncomplete ? (
        <InlineNotice tone="attention">
          <strong>{t("cleanup.blocked.title")}</strong>
          <p>{t("cleanup.blocked.body")}</p>
        </InlineNotice>
      ) : null}

      <Panel
        id="opportunities-heading"
        title={t("cleanup.opportunities.title")}
        description={t("cleanup.opportunities.description")}
        actions={
          <span className="cs-eyebrow">
            <DataValue partial={partial}>
              {tCount(data.items.length, "unit.opportunity.one", "unit.opportunity.other")}
            </DataValue>
          </span>
        }
      >
        {data.items.length === 0 ? (
          <EmptyState
            title={t("cleanup.empty.noScanTitle")}
            body={
              data.lastScanAt === null
                ? t("cleanup.empty.noScanDescription")
                : t("cleanup.empty.noItemsDescription")
            }
            next={
              data.lastScanAt === null
                ? t("cleanup.empty.noScanNext")
                : t("cleanup.empty.noItemsNext")
            }
          />
        ) : (
          <ul className="cleanup-list">
            {data.items.map((item) => {
              const expanded = expandedId === item.id;
              const reason = blockingReason(item, scanIncomplete);
              const blockedId = `cleanup-blocked-${item.id}`;
              return (
                <li
                  className={`cleanup-item cleanup-item--${item.tier.toLowerCase().replace("_", "-")}`}
                  key={item.id}
                >
                  <span className="cleanup-item__rail" aria-hidden="true" />
                  <div className="cleanup-item__body">
                    <header className="cleanup-item__head">
                      <div className="cleanup-item__title">
                        <Tier tier={item.tier} />
                        <h3>{item.title}</h3>
                        <p>{item.whatItIs}</p>
                      </div>
                      <div className="cleanup-item__recoverable">
                        <span>{t("cleanup.card.expectedRecovery")}</span>
                        <span className="cleanup-item__amount">
                          <Bytes value={item.recoverableBytes} partial={partial} />
                        </span>
                      </div>
                      <Button
                        variant="ghost"
                        className="cleanup-item__toggle"
                        aria-expanded={expanded}
                        aria-controls={`cleanup-detail-${item.id}`}
                        aria-label={
                          expanded
                            ? t("cleanup.card.hideDetailsFor", { title: item.title })
                            : t("cleanup.card.readDetailsFor", { title: item.title })
                        }
                        onClick={() => setExpandedId(expanded ? null : item.id)}
                      >
                        {expanded
                          ? t("cleanup.card.hideDetails")
                          : t("cleanup.card.readDetails")}
                      </Button>
                    </header>

                    <div className="cleanup-item__quick">
                      <span>
                        {item.estimatedAgeDays === null ? (
                          <UnknownValue label={t("cleanup.card.ageUnknown")} />
                        ) : (
                          <DataValue partial={partial}>
                            {t("cleanup.card.oldestEvidence", {
                              age: formatAge(item.estimatedAgeDays)
                            })}
                          </DataValue>
                        )}
                      </span>
                      <span>
                        {item.itemCount === null ? (
                          <UnknownValue />
                        ) : (
                          <DataValue partial={partial}>{entryCount(item.itemCount)}</DataValue>
                        )}
                      </span>
                      {item.recoverableBytes === null ? (
                        <span>{t("cleanup.card.recoveryUnknown")}</span>
                      ) : null}
                    </div>

                    {expanded ? (
                      <div className="cleanup-item__detail" id={`cleanup-detail-${item.id}`}>
                        <div className="explanation-grid">
                          <section>
                            <h4>{t("cleanup.card.why")}</h4>
                            <p>{item.why}</p>
                          </section>
                          <section>
                            <h4>{t("cleanup.card.stays")}</h4>
                            <p>{item.whatStaysUntouched}</p>
                          </section>
                          <section>
                            <h4>{t("cleanup.card.consequence")}</h4>
                            <p>{item.consequence}</p>
                          </section>
                        </div>

                        {item.evidence.length > 0 ? (
                          <ul className="evidence-list" aria-label={t("cleanup.card.evidence")}>
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

                        <div className="cleanup-item__action">
                          <div>
                            <strong>{t("cleanup.card.previewMandatory")}</strong>
                            <p>{t("cleanup.card.previewMandatoryBody")}</p>
                            {reason ? (
                              <p className="cleanup-item__blocked" id={blockedId}>
                                {reason}
                              </p>
                            ) : null}
                          </div>
                          <Button
                            variant="primary"
                            disabled={reason !== null}
                            aria-describedby={reason ? blockedId : undefined}
                            onClick={() => onPreview(item)}
                          >
                            {t("cleanup.card.previewButton")}
                          </Button>
                        </div>
                      </div>
                    ) : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </Panel>

      <Panel
        id="offlimits-heading"
        title={t("cleanup.offlimits.title")}
        description={t("cleanup.offlimits.description")}
        actions={
          <span className="cs-eyebrow">
            {tCount(
              data.offLimits.length,
              "unit.excludedCategory.one",
              "unit.excludedCategory.other"
            )}
          </span>
        }
        className="panel--offlimits"
      >
        {data.offLimits.length === 0 ? (
          <p className="cs-micro">
            {data.lastScanAt === null
              ? t("cleanup.offlimits.emptyNoScan")
              : t("cleanup.offlimits.empty")}
          </p>
        ) : (
          <ul className="offlimits-list">
            {data.offLimits.map((item) => (
              <li key={item.id}>
                <div className="offlimits-list__title">
                  <Tier tier={item.tier} />
                  <h3>{item.title}</h3>
                </div>
                <p>{item.whatItIs}</p>
                <p className="offlimits-list__reason">
                  <strong>{t("cleanup.offlimits.why")}</strong>{" "}
                  {item.offLimitsReason ?? t("cleanup.offlimits.fallbackReason")}
                </p>
                <p className="cs-micro">
                  {item.observedBytes === null ? (
                    t("cleanup.offlimits.sizeUnknown")
                  ) : (
                    <DataValue partial={partial}>
                      {t("cleanup.offlimits.observed", {
                        size: formatBytes(item.observedBytes)
                      })}
                    </DataValue>
                  )}
                </p>
              </li>
            ))}
          </ul>
        )}
      </Panel>
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
        eyebrow={t("cleanup.preview.pageEyebrow")}
        title={t("cleanup.preview.title", { title: item.title })}
        description={t("cleanup.preview.description")}
      />
      <Button variant="ghost" className="back-link" onClick={onBack}>
        <span aria-hidden="true">←</span> {t("cleanup.preview.back")}
      </Button>

      {previewState.status === "loading" ? (
        <div role="status" aria-live="polite">
          <Surface tone="sunken" padded className="preview-loading">
            <span className="cs-spinner" aria-hidden="true" />
            <div>
              <h2>{t("cleanup.preview.building")}</h2>
              <p>{t("cleanup.preview.buildingBody")}</p>
            </div>
          </Surface>
        </div>
      ) : null}

      {previewState.status === "error" ? <InlineError error={previewState.error} /> : null}

      {preview ? (
        <>
          <CoverageBanner
            complete={preview.coverage.completeness === "complete"}
            coverage={preview.coverage}
            context="cleanupPreview"
          />
          <article className="preview-sheet" aria-labelledby="preview-heading">
            <header className="preview-sheet__header">
              <p className="cs-eyebrow">{t("cleanup.preview.eyebrow")}</p>
              <h2 id="preview-heading">{item.title}</h2>
              <p>{preview.consequence}</p>
              <div className="preview-sheet__total">
                <span>{t("cleanup.preview.wouldMove")}</span>
                <span className="preview-sheet__amount cs-num">
                  <Bytes
                    value={preview.totalBytes}
                    partial={preview.coverage.completeness !== "complete"}
                  />
                </span>
                <span className="cs-micro">
                  {tCount(preview.items.length, "unit.previewLine.one", "unit.previewLine.other")}
                </span>
              </div>
            </header>

            <div className="preview-columns">
              <section aria-labelledby="what-moves-heading">
                <h3 id="what-moves-heading">{t("cleanup.preview.heading")}</h3>
                <ul className="preview-items">
                  {preview.items.map((previewItem) => (
                    <li key={previewItem.title}>
                      <div>
                        <strong>{previewItem.title}</strong>
                        <p>{previewItem.reason}</p>
                      </div>
                      <div className="preview-items__numbers">
                        <Bytes
                          value={previewItem.bytes}
                          partial={preview.coverage.completeness !== "complete"}
                        />
                        <span>{entryCount(previewItem.itemCount)}</span>
                      </div>
                    </li>
                  ))}
                </ul>
              </section>

              <section aria-labelledby="where-heading">
                <h3 id="where-heading">{t("cleanup.preview.where")}</h3>
                <p className="preview-route">
                  {preview.destination ?? t("cleanup.preview.destinationUnknown")}
                </p>
                <h3>{t("cleanup.preview.undo")}</h3>
                <p>{preview.howToUndo ?? t("cleanup.preview.undoUnknown")}</p>
                <h3>{t("cleanup.preview.stays")}</h3>
                <ul className="stays-list">
                  {preview.whatStaysUntouched.map((statement) => (
                    <li key={statement}>{statement}</li>
                  ))}
                </ul>
              </section>
            </div>

            {preview.warnings.length > 0 ? (
              <div className="preview-warnings" role="note">
                <strong>{t("cleanup.preview.warnings")}</strong>
                <ul>
                  {preview.warnings.map((warning) => (
                    <li key={warning}>{warning}</li>
                  ))}
                </ul>
              </div>
            ) : null}

            {!preview.complete || scanIncomplete ? (
              <InlineNotice tone="attention">
                <strong>{t("cleanup.preview.cannotConfirm")}</strong>
                <p>{t("cleanup.preview.cannotConfirmBody")}</p>
              </InlineNotice>
            ) : null}

            {actionError ? <InlineError error={actionError} /> : null}

            <div className="confirm-zone">
              <label className="confirm-check">
                <input
                  type="checkbox"
                  checked={acknowledged}
                  aria-label={t("cleanup.confirm.acknowledgementAria")}
                  onChange={(event) => setAcknowledged(event.target.checked)}
                  disabled={!preview.complete || scanIncomplete || executing}
                />
                <span>{t("cleanup.confirm.acknowledgement")}</span>
              </label>
              <Button
                variant="primary"
                disabled={confirmBlocked}
                onClick={() => onConfirm(preview)}
              >
                {executing ? t("cleanup.confirm.working") : t("cleanup.confirm.button")}
              </Button>
              <p className="cs-micro">{t("cleanup.confirm.microcopy")}</p>
            </div>
            <div className="visually-hidden" role="status" aria-live="polite">
              {executing
                ? t("cleanup.confirm.liveConfirming")
                : acknowledged
                  ? t("cleanup.confirm.liveChecked")
                  : ""}
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
        eyebrow={t("cleanup.result.eyebrow")}
        title={t("cleanup.result.title")}
        description={t("cleanup.result.description", { title: itemTitle })}
      />
      <Surface tone="raised" padded className="result-panel">
        <span className="result-panel__mark" aria-hidden="true">
          ✓
        </span>
        <p className="cs-eyebrow">{t("cleanup.result.panelEyebrow")}</p>
        <h2>
          {execution.simulated ? t("cleanup.result.simulated") : t("cleanup.result.accepted")}
        </h2>
        <p>{execution.note}</p>
        <dl className="result-facts">
          <div>
            <dt>{t("cleanup.result.freedBytes")}</dt>
            <dd>
              {execution.freedBytes === null ? (
                <UnknownValue />
              ) : (
                formatBytes(execution.freedBytes)
              )}
            </dd>
          </div>
          <div>
            <dt>{t("cleanup.result.entriesConfirmed")}</dt>
            <dd>
              {execution.movedItems === null ? (
                <UnknownValue />
              ) : (
                entryCount(execution.movedItems)
              )}
            </dd>
          </div>
          <div>
            <dt>{t("cleanup.result.verification")}</dt>
            <dd>{execution.verification}</dd>
          </div>
        </dl>
        {execution.leftovers.length > 0 ? (
          <InlineNotice tone="attention">
            <strong>{t("cleanup.result.leftovers")}</strong>
            <ul>
              {execution.leftovers.map((leftover) => (
                <li key={leftover}>{leftover}</li>
              ))}
            </ul>
          </InlineNotice>
        ) : null}
        <p className="cs-micro">{t("cleanup.result.microcopy")}</p>
        <Button variant="primary" onClick={onReturn}>
          {t("cleanup.result.return")}
        </Button>
      </Surface>
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
  const [resultTitle, setResultTitle] = useState<string>(t("cleanup.result.defaultItem"));
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
      setResultTitle(
        previewState?.status === "ready"
          ? previewState.item.title
          : t("cleanup.result.defaultItem")
      );
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
  const eyebrow = t("cleanup.eyebrow");
  const title = t("cleanup.title");
  const description = t("cleanup.description");

  if (state.status === "loading") {
    return (
      <ScreenLoading
        eyebrow={eyebrow}
        title={title}
        description={description}
        layout="cards"
      />
    );
  }
  if (state.status === "error") {
    return (
      <ScreenError
        eyebrow={eyebrow}
        title={title}
        description={description}
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
