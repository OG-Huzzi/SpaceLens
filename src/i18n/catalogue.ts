/**
 * The typed message catalogue.
 *
 * `en` is the base locale and the source of truth for the key union:
 * `MessageKey` is `keyof typeof en`, so an unknown key is a TypeScript error at
 * the call site (`t("no.such.key")`), not a runtime fallback. `de` is typed as
 * `Record<MessageKey, string>`, so a missing German string fails the build too.
 *
 * TRANSLATION RULE: a string that cannot be translated confidently stays in
 * English in the `de` catalogue and is marked with an `// EN:` comment saying
 * why. Nothing is machine-guessed and no term is invented to look complete.
 * Product names and established technical loanwords (CoreSight, Scan, Engine,
 * IPC, Mock) stay as they are: that is how German technical UI actually reads,
 * not text left untranslated by accident.
 */

export const en = {
  // ---------------------------------------------------------------------
  // App shell
  // ---------------------------------------------------------------------
  "app.skipToContent": "Skip to content",
  "app.brand.home": "CoreSight home",
  "app.nav.label": "Primary",
  "app.nav.home": "Home",
  "app.nav.storage": "Storage",
  "app.nav.cleanup": "Cleanup",
  "app.nav.history": "History",
  "app.nav.drives": "Drives",
  "app.nav.settings": "Settings",
  "app.header.engineLive": "Local engine",
  "app.header.enginePreview": "Local preview",
  "app.header.engineLiveTitle":
    "Reading the local CoreSight engine through the coresight.v1 contract.",
  "app.header.enginePreviewTitle":
    "Rendering deterministic local fixture data; no engine is attached.",
  "app.footer.privacy":
    "CoreSight analyses storage locally. No paths, names, or hashes leave this device.",
  "app.footer.build": "Phase 8 UI shell · contract {contract} · {mode}",
  "app.footer.modeLive": "live local engine",
  "app.footer.modePreview": "deterministic mock data",

  // ---------------------------------------------------------------------
  // Contract mismatch (the shell refusing to read an unknown contract)
  // ---------------------------------------------------------------------
  "contract.eyebrow": "Contract check",
  "contract.title": "This UI cannot read the installed data contract",
  "contract.bodyBefore": "The locally returned contract is ",
  "contract.bodyBetween": ", while this shell supports ",
  "contract.bodyAfter": ". CoreSight refuses to guess at incompatible fields.",
  "contract.panelTitle": "No screens were populated",
  "contract.panelBody":
    "Update the app so the frontend and engine share a compatible contract version. No data was changed and no cleanup path was opened.",

  // ---------------------------------------------------------------------
  // Ambient scan status (the read-only "widget" strip)
  // ---------------------------------------------------------------------
  "scan.label": "Scan status",
  "scan.loading": "Reading scan status…",
  "scan.unavailable": "Scan status unavailable",
  "scan.idle": "No completed scan yet",
  "scan.scanning": "Scanning · {seen} seen",
  "scan.scanningInArea": "Scanning · {seen} seen · {area}",
  "scan.failed": "Last scan failed · figures may be stale",
  "scan.partial": "Last scan completed with limits · {count} could not be read",
  "scan.complete": "Scan complete · {time}",
  "scan.advanced": "{bytes} seen this scan · id {id}",
  "scan.advancedBytesUnknown": "Byte count not reported · id {id}",

  // ---------------------------------------------------------------------
  // Shared empty / loading / error state components
  // ---------------------------------------------------------------------
  "state.unknown": "Unknown",
  "state.partialTag.label": "partial",
  "state.partialTag.title":
    "This figure comes from a partial source; it is not a complete total.",
  "state.coverage.incomplete.scan": "This scan is incomplete",
  "state.coverage.incomplete.storageScan": "This storage scan is incomplete",
  "state.coverage.incomplete.cleanupPreview": "This cleanup preview is incomplete",
  "state.coverage.incomplete.duplicateAnalysis": "This duplicate analysis is incomplete",
  "state.coverage.incomplete.largeFileResult": "This large-file result is incomplete",
  "state.coverage.outsideHeading": "Some entries are outside this result",
  "state.coverage.incompleteBody":
    "Totals here are lower bounds, not complete totals. Aggregate figures from this source carry a partial label.",
  "state.coverage.outsideBody":
    "The figures below describe only the entries the engine could evaluate.",
  "state.coverage.unreadableLabel": "Unreadable entries:",
  "state.coverage.unknownSizesLabel": "Unknown sizes:",
  "state.coverage.unreadableAria": "Unreadable entries: {value}",
  "state.coverage.unknownSizesAria": "Unknown sizes: {value}",
  "state.coverage.notIncluded": "Not included:",
  "state.loading.heading": "Reading the local snapshot",
  "state.loading.body":
    "Large scans can take a moment. This screen will update when the data is ready.",
  "state.loading.label": "Loading",
  "state.error.eyebrow": "Could not load this screen",
  "state.error.nothingChanged":
    "Nothing was changed. Leave this screen and open it again; if the snapshot is still unavailable, restart CoreSight.",
  "state.error.returnHome": "Return to Home",
  "state.error.reference": "Reference",
  "state.sizeBar.unknown": "{label}: share unknown",
  "state.sizeBar.share": "{label}: {percent}",
  "state.sizeBar.partialSuffix": ", from a partial source",

  // ---------------------------------------------------------------------
  // Safety tiers
  // ---------------------------------------------------------------------
  "tier.safe": "Safe",
  "tier.review": "Review",
  "tier.offLimits": "Off-limits",

  // ---------------------------------------------------------------------
  // Counted units (plural pairs used with `tCount`)
  // ---------------------------------------------------------------------
  "unit.entry.one": "{count} entry",
  "unit.entry.other": "{count} entries",
  "unit.opportunity.one": "{count} opportunity",
  "unit.opportunity.other": "{count} opportunities",
  "unit.previewLine.one": "{count} preview line",
  "unit.previewLine.other": "{count} preview lines",
  "unit.excludedCategory.one": "{count} excluded category",
  "unit.excludedCategory.other": "{count} excluded categories",

  // ---------------------------------------------------------------------
  // Cleanup — the only screen with actions. Safety copy is localised first.
  // ---------------------------------------------------------------------
  "cleanup.eyebrow": "Cleanup",
  "cleanup.title": "What can I safely remove?",
  "cleanup.description":
    "Ranked opportunities with their reasoning attached. One review, one preview, one explicit confirmation.",
  "cleanup.stamp.lastScan": "Last scan",
  "cleanup.stamp.partial": "Partial scan · cleanup actions blocked",
  "cleanup.stamp.actionsHere": "Actions available on this screen only",
  "cleanup.summary.safeEyebrow": "Safe estimate",
  "cleanup.summary.safeBody":
    "Across safe-tier opportunities. Nothing is moved automatically.",
  "cleanup.summary.reviewEyebrow": "Needs review",
  "cleanup.summary.reviewBody":
    "Review-only opportunities, never bundled into a safe action.",
  "cleanup.rules.title": "No clean-everything path exists.",
  "cleanup.rules.body":
    "Each opportunity has its own preview. Off-limits categories stay visible below with the reason they cannot be selected.",
  "cleanup.blocked.title": "Actions are disabled while this scan is incomplete.",
  "cleanup.blocked.body":
    "Safe-tier plans require complete knowledge. The opportunities remain visible so you can inspect what CoreSight found, but nothing can be previewed for confirmation.",
  "cleanup.opportunities.title": "Opportunities",
  "cleanup.opportunities.description":
    "Engine order. Expand one item to read why it is suggested, what stays untouched, and what happens next.",
  "cleanup.empty.noScanTitle": "No cleanup opportunities right now",
  "cleanup.empty.noScanDescription":
    "There is no completed scan to produce cleanup opportunities. This is an absence of data, not a claim that nothing can be cleaned.",
  "cleanup.empty.noScanNext":
    "Once a complete scan finishes, safe and review opportunities will appear here with their reasoning.",
  "cleanup.empty.noItemsDescription":
    "The last completed scan found nothing eligible for safe or review cleanup. This is an empty result, not a hidden recommendation.",
  "cleanup.empty.noItemsNext":
    "Keep the off-limits summary below for context. New opportunities appear after the next completed scan.",
  "cleanup.card.readDetails": "Read the details",
  "cleanup.card.hideDetails": "Hide details",
  "cleanup.card.readDetailsFor": "Read details for {title}",
  "cleanup.card.hideDetailsFor": "Hide details for {title}",
  "cleanup.card.expectedRecovery": "Expected recovery",
  "cleanup.card.ageUnknown": "Age unknown",
  "cleanup.card.oldestEvidence": "Oldest evidence: {age}",
  "cleanup.card.recoveryUnknown": "Recovery amount unknown",
  "cleanup.card.why": "Why CoreSight suggests this",
  "cleanup.card.stays": "What stays untouched",
  "cleanup.card.consequence": "What happens after removal",
  "cleanup.card.evidence": "Evidence",
  "cleanup.card.previewMandatory": "Preview is mandatory.",
  "cleanup.card.previewMandatoryBody":
    "The next step shows exactly what would move, where it would go, and how to undo it. No file moves until you confirm.",
  "cleanup.card.previewButton": "Preview what would move",
  "cleanup.offlimits.title": "Visibly excluded",
  "cleanup.offlimits.description":
    "Off-limits categories are not hidden. They are shown so the boundary is clear, with no action path.",
  "cleanup.offlimits.emptyNoScan":
    "No off-limits categories are available without a completed scan.",
  "cleanup.offlimits.empty": "No off-limits categories were reported for this drive.",
  "cleanup.offlimits.why": "Why it is excluded:",
  "cleanup.offlimits.fallbackReason": "Protected by engine policy.",
  "cleanup.offlimits.sizeUnknown":
    "Size unknown — this category is never counted as reclaimable.",
  "cleanup.offlimits.observed":
    "{size} observed. This is context, not recoverable space.",
  "cleanup.blocking.scanIncomplete":
    "Cleanup is blocked because the scan is incomplete. A complete scan is required before any file can move.",
  "cleanup.blocking.noPreview":
    "This item has no complete preview route and cannot be confirmed.",
  "cleanup.blocking.unknownBytes":
    "The recoverable amount is unknown, so CoreSight cannot build a complete preview.",
  "cleanup.preview.eyebrow": "Exact preview · nothing has moved",
  "cleanup.preview.pageEyebrow": "Cleanup · Preview",
  "cleanup.preview.title": "Review: {title}",
  "cleanup.preview.description":
    "Step two of two. This is the exact statement you confirm; nothing has moved yet.",
  "cleanup.preview.back": "Back to opportunities",
  "cleanup.preview.building": "Building the exact preview",
  "cleanup.preview.buildingBody":
    "Checking the selected items, destination, undo route, and byte accounting.",
  "cleanup.preview.wouldMove": "Would be moved",
  "cleanup.preview.heading": "Exactly what would move",
  "cleanup.preview.where": "Where it goes",
  "cleanup.preview.undo": "How to undo",
  "cleanup.preview.stays": "What stays untouched",
  "cleanup.preview.destinationUnknown": "Destination unknown — confirmation is blocked.",
  "cleanup.preview.undoUnknown": "Undo route unknown — confirmation is blocked.",
  "cleanup.preview.warnings": "Warnings to read before confirming",
  "cleanup.preview.cannotConfirm": "This preview cannot be confirmed.",
  "cleanup.preview.cannotConfirmBody":
    "A destination, undo route, byte amount, or complete scan is missing. CoreSight blocks the plan rather than offering a partial confirmation.",
  "cleanup.confirm.acknowledgementAria":
    "I have read exactly what would move, where it would go, and how to undo it.",
  "cleanup.confirm.acknowledgement":
    "I have read exactly what would move, where it would go, and how to undo it. I understand that Trash only protects these items until Trash is emptied.",
  "cleanup.confirm.button": "Confirm move to Trash",
  "cleanup.confirm.working": "Confirming…",
  "cleanup.confirm.microcopy":
    "One opportunity at a time. No action is taken for any other cleanup item.",
  "cleanup.confirm.liveConfirming": "Confirming cleanup",
  "cleanup.confirm.liveChecked": "Confirmation checked",
  "cleanup.result.eyebrow": "Cleanup · Confirmation",
  "cleanup.result.panelEyebrow": "Result",
  "cleanup.result.title": "Confirmation recorded",
  "cleanup.result.description": "The cleanup for “{title}” passed through the preview gate.",
  "cleanup.result.simulated": "UI preview completed in memory",
  "cleanup.result.accepted": "Cleanup request accepted",
  "cleanup.result.freedBytes": "Freed bytes",
  "cleanup.result.entriesConfirmed": "Entries confirmed",
  "cleanup.result.verification": "Verification",
  "cleanup.result.leftovers": "Leftovers were reported",
  "cleanup.result.microcopy":
    "No permanent deletion is offered. In this Phase 8 shell the mock adapter does not touch the filesystem; Phase 9 wires the authorized IPC action.",
  "cleanup.result.return": "Review remaining opportunities",
  "cleanup.result.defaultItem": "selected cleanup",
  "cleanup.error.message": "This action could not be completed.",
  "cleanup.error.detail":
    "Nothing was moved. Return to the opportunities list and try again.",

  // ---------------------------------------------------------------------
  // Settings
  // ---------------------------------------------------------------------
  "settings.eyebrow": "Settings",
  "settings.title": "Preferences",
  "settings.description":
    "How CoreSight looks and behaves on this device. These preferences are stored locally and never change what CoreSight is allowed to do.",
  "settings.language.title": "Language",
  "settings.language.description":
    "Interface strings only. Every built-in interface text in this build is translated; labels supplied by the engine are shown verbatim.",
  "settings.language.label": "Interface language",
  "settings.language.en": "English",
  "settings.language.de": "Deutsch",
  "settings.appearance.title": "Appearance",
  "settings.appearance.description":
    "Theme, density, and motion. Theme and motion follow the operating system until you choose otherwise.",
  "settings.theme.legend": "Theme",
  "settings.theme.system": "System",
  "settings.theme.light": "Light",
  "settings.theme.dark": "Dark",
  "settings.density.legend": "Density",
  "settings.density.comfortable": "Comfortable",
  "settings.density.compact": "Compact",
  "settings.motion.legend": "Motion",
  "settings.motion.system": "Follow system",
  "settings.motion.reduce": "Reduce motion",
  "settings.motion.allow": "Allow motion",
  "settings.motion.hint":
    "Reduce motion disables non-essential animation. Allow motion overrides an operating-system reduce-motion setting.",
  "settings.screens.title": "Screens",
  "settings.screens.description": "Choose which destinations appear in the navigation.",
  "settings.screens.hiddenNote":
    "Hiding a screen is a view preference, never a capability change. Every state-changing action still lives in Cleanup, behind a preview and an explicit confirmation, and nothing here can enable or disable one.",
  "settings.screens.lastVisible": "At least one screen must stay visible.",
  "settings.details.title": "Details",
  "settings.details.showAdvanced": "Show advanced details",
  "settings.details.showAdvancedHint":
    "Adds diagnostic detail to the scan strip, such as byte counts and the scan identifier.",
  "settings.about.title": "About these preferences",
  "settings.about.description":
    "Preferences are pure client state. They live in this browser profile and are never sent to the engine or anywhere else.",
  "settings.about.storageKey": "Storage key",
  "settings.about.schemaVersion": "Schema version",
  "settings.about.locale": "Active locale",
  "settings.about.reset": "Restore default preferences",
  "settings.about.resetHint":
    "Resets these preferences only. No files, scan data, snapshots, or history are touched.",
  "settings.about.storageUnavailable":
    "This browser refused to store preferences. Changes apply for this session only.",
  "settings.about.saved": "Preferences saved on this device.",

  // ---------------------------------------------------------------------
  // Formatting sentinels and relative time
  //
  // These are the words `src/lib/format.ts` renders. They stay out of the
  // screens on purpose: a formatter that returns "Unknown" is an English
  // literal leaking into every locale.
  // ---------------------------------------------------------------------
  "format.unknownChange": "Unknown change",
  "format.noChange": "No change",
  "format.ageUnknown": "Age unknown",
  "format.lessThanADay": "Less than a day",
  "format.etaUnknown": "Time remaining unknown",
  "format.etaSeconds": "{count} sec remaining",
  "format.etaMinutes": "{count} min remaining",
  "format.shareUnknown": "Share unknown",
  "format.shareOfParent": "{percent} of parent",

  // ---------------------------------------------------------------------
  // Shared status words and counted units for the four informational screens
  // ---------------------------------------------------------------------
  "status.complete": "Complete",
  "status.partial": "Partial",
  "status.partialComparison": "Partial comparison",
  "status.comparableCoverage": "Comparable coverage",
  "status.baselineOnly": "Baseline only",

  "drive.health.healthy": "Healthy",
  "drive.health.attention": "Needs attention",
  "drive.health.unknown": "Health unknown",
  "drive.connection.connected": "Connected",
  "drive.connection.remembered": "Remembered",
  "drive.connection.rememberedDisconnected": "Remembered · disconnected",
  "drive.kind.internal": "Internal drive",
  "drive.kind.external": "External drive",
  "drive.kind.startupVolume": "Startup volume",

  "unit.day.one": "{count} day",
  "unit.day.other": "{count} days",
  "unit.subcategory.one": "{count} subcategory",
  "unit.subcategory.other": "{count} subcategories",
  "unit.label.one": "{count} label",
  "unit.label.other": "{count} labels",
  "unit.observedEntry.one": "{count} observed entry",
  "unit.observedEntry.other": "{count} observed entries",
  "unit.category.one": "{count} category",
  "unit.category.other": "{count} categories",
  "unit.topLevelGroup.one": "{count} top-level group",
  "unit.topLevelGroup.other": "{count} top-level groups",
  "unit.group.one": "{count} group",
  "unit.group.other": "{count} groups",
  "unit.verifiedCopy.one": "{count} verified copy",
  "unit.verifiedCopy.other": "{count} verified copies",
  "unit.member.one": "{count} member",
  "unit.member.other": "{count} members",
  "unit.snapshot.one": "{count} snapshot",
  "unit.snapshot.other": "{count} snapshots",
  "unit.drive.one": "{count} drive",
  "unit.drive.other": "{count} drives",
  "unit.connectedDrive.one": "{count} connected drive",
  "unit.connectedDrive.other": "{count} connected drives",
  "unit.match.one": "{count} match",
  "unit.match.other": "{count} matches",
  "unit.opportunityToReview.one": "{count} opportunity to review",
  "unit.opportunityToReview.other": "{count} opportunities to review",

  // ---------------------------------------------------------------------
  // Home
  // ---------------------------------------------------------------------
  "home.eyebrow": "Home",
  "home.title": "How is your storage doing?",
  "home.description":
    "A calm reading of the last local snapshot. Context first, technical detail later.",
  "home.empty.description":
    "One calm summary of the drives CoreSight can see, with every number tied to its context.",
  "home.empty.title": "No drive summary yet",
  "home.empty.body":
    "CoreSight has not completed a storage scan on this device. Nothing is being measured in the background.",
  "home.empty.next":
    "Once a scan completes, this page will show free space, the largest categories, and what changed.",
  "home.stamp.lastScan": "Last completed scan",
  "home.stamp.partial": "Partial scan · totals incomplete",
  "home.stamp.localOnly": "Local only · never uploaded",
  "home.safeToReclaim": "Safe to reclaim",
  "home.reviewAmountUnknown": "review amount unknown",
  "home.reviewInOpportunities": "{bytes} in review-only opportunities",
  "home.hero.countUnknown": "Opportunity count unknown",
  "home.basisComplete": "based on the scan completed {time}.",
  "home.basisPartial": "based on the partial scan completed {time}.",
  "home.rule":
    "Nothing moves automatically. Every cleanup is previewed and confirmed one opportunity at a time.",
  "home.cleanupBlocked":
    "This scan is incomplete, so cleanup is blocked until a complete scan succeeds.",
  "home.cta.openCleanup": "Open cleanup review",
  "home.cta.reviewSafe": "Review {bytes} you can safely remove",
  "home.driveHealth": "Drive health",
  "home.drive.lastKnownSuffix": " · last known",
  "home.drive.usedSpaceLabel": "{name} used space",
  "home.drive.free": "{bytes} free",
  "home.drive.freeUnknownValue": "Free space unknown",
  "home.drive.usedOfTotal": "{used} used of {total}",
  "home.drive.freeUnknown":
    "Free space is unknown while this drive is disconnected, not zero.",
  "home.categories.title": "Largest categories",
  "home.categories.description":
    "Human categories first. Open Storage for subcategories and sample locations.",
  "home.categories.explore": "Explore storage",
  "home.category.ofUsedSpace": "{percent} of used space",
  "home.category.shareLabel": "{name} share of used space",
  "home.since.title": "Since last scan",
  "home.since.description":
    "The previous snapshot could not be compared, so no change is claimed.",
  "home.change.across": "{delta} across the categories you can see",
  "home.change.unknown": "Change unknown",
  "home.change.unknownBody":
    "Scan twice to establish a comparison. An absent comparison is not the same as “nothing changed”.",
  "home.change.viewHistory": "View scan history",
  "home.change.seeWhatChanged": "See what changed",
  "home.caveat.aria": "Snapshot caveat",
  "home.caveat.title": "How to read these numbers.",
  "home.caveat.body":
    "Sizes are observed logical bytes, not promises of recoverable space. A category delta is labelled partial whenever either snapshot was incomplete. Unknown values stay unknown.",

  // ---------------------------------------------------------------------
  // Storage
  // ---------------------------------------------------------------------
  "storage.eyebrow": "Storage",
  "storage.title": "What's using my space?",
  "storage.description":
    "Human categories first, technical labels one deliberate step down.",
  "storage.descriptionMain":
    "Start with human categories. Expand only when you want the evidence underneath.",
  "storage.empty.title": "No storage categories yet",
  "storage.empty.body":
    "There is no completed category breakdown for this device. Zero categories is a real empty result, not an unknown total.",
  "storage.empty.next":
    "After a scan completes, categories, verified duplicates, and large files will appear here.",
  "storage.summary.observedTotal": "Observed total",
  "storage.summary.bodyComplete":
    "Across {count}. This is not free space; it is what the scan could observe.",
  "storage.summary.bodyPartial":
    "Across {count}. This is not free space; it is what the partial scan could observe.",
  "storage.summary.categories": "Categories",
  "storage.summary.entriesWithoutSize": "Entries without a size",
  "storage.summary.duplicateEvidence": "Duplicate evidence",
  "storage.categories.title": "Categories",
  "storage.categories.description":
    "Sorted by observed size. Raw locations live only inside the deepest disclosure.",
  "storage.search.label": "Search categories",
  "storage.search.aria": "Search storage categories",
  "storage.search.placeholder": "Games, Documents, Development…",
  "storage.search.help": "Search by human category name, not by path.",
  "storage.search.status": "{matches} for “{query}”.",
  "storage.search.emptyTitle": "No matching categories",
  "storage.search.emptyBody":
    "No human category contains “{query}”. This search does not guess at path fragments.",
  "storage.search.emptyNext":
    "Try a broader category name, or clear the search to see every category.",
  "storage.column.category": "Category",
  "storage.column.shareOfTotal": "Share of total",
  "storage.column.observed": "Observed",
  "storage.column.share": "Share",
  "storage.column.change": "Change",
  "storage.category.summary":
    "{name}: {bytes} observed, {percent} of parent, {delta} since the last scan{partial}. {children}.",
  "storage.category.shareLabel": "{name} share",
  "storage.category.noDeeperBreakdown": "No deeper breakdown",
  "storage.category.noDeeperBody": "No deeper category breakdown was recorded.",
  "storage.category.entries": "Entries",
  "storage.category.entriesWithoutSize": "Entries without a known size",
  "storage.category.observationErrors": "Observation errors",
  "storage.category.sampleLocations": "Sample locations",
  "storage.category.sampleLocationsAria": "Sample locations for {name}",
  "storage.category.sampleLocationsNote":
    "Labels are supplied by the engine and displayed verbatim. CoreSight does not build or resolve paths in the interface.",
  "storage.duplicates.title": "Verified duplicate groups",
  "storage.duplicates.description":
    "Only SHA-256 matches with equal sizes are called duplicates. Hard-link aliases are shown, never counted as savings.",
  "storage.duplicates.recoverable": "Recoverable, verified",
  "storage.duplicates.empty": "No hash-verified duplicate groups were found.",
  "storage.duplicates.group": "Group {index}",
  "storage.duplicates.groupHeading": "{count} at {bytes} each",
  "storage.duplicates.recoverableValue": "Recoverable {bytes}",
  "storage.duplicates.verified": "SHA-256 verified",
  "storage.duplicates.hashLabel": "Hash",
  "storage.duplicates.showMembersAria":
    "Show member labels for duplicate group {index}",
  "storage.duplicates.showMembers": "Show member labels",
  "storage.duplicates.hardlinkAlias": " · hard-link alias",
  "storage.duplicates.hardlinkNote":
    "A hard-link alias is another name for the same filesystem object. Removing it frees no space, so it contributes nothing to recoverable bytes.",
  "storage.duplicates.unverified.one":
    "{count} file lacks a verified content hash and is excluded from every duplicate claim.",
  "storage.duplicates.unverified.other":
    "{count} files lack a verified content hash and are excluded from every duplicate claim.",
  "storage.duplicates.unknownSize.one":
    "{count} entry has a verified hash but no observed size; size equality cannot be proven, so they are never grouped.",
  "storage.duplicates.unknownSize.other":
    "{count} entries have a verified hash but no observed size; size equality cannot be proven, so they are never grouped.",
  "storage.bigFiles.title": "Large files",
  "storage.bigFiles.description":
    "Files above {bytes}. Unknown age is never treated as old.",
  "storage.bigFiles.empty": "No files crossed the current size threshold.",
  "storage.bigFiles.categoryUnknown": "Category unknown",
  "storage.bigFiles.age": "{age} old",
  "storage.bigFiles.modifiedUnknown": "Modified time unknown",
  "storage.bigFiles.modified": "Modified {time}",
  "storage.bigFiles.showLocationAria":
    "Show location label for the {bytes} file",
  "storage.bigFiles.showLocation": "Show location label",
  "storage.bigFiles.unknownAge.one":
    "{count} entry had an unknown modification time and was excluded from age-based conclusions.",
  "storage.bigFiles.unknownAge.other":
    "{count} entries had an unknown modification time and were excluded from age-based conclusions.",

  // ---------------------------------------------------------------------
  // History
  // ---------------------------------------------------------------------
  "history.eyebrow": "History",
  "history.title": "What changed?",
  "history.description":
    "Snapshots answer the +80 GB question with dated, comparable evidence — never with a guessed trend.",
  "history.stamp.title": "Snapshots stored locally",
  "history.stamp.note": "Local database · nothing uploaded",
  "history.empty.title": "No snapshots yet",
  "history.empty.body":
    "There is no history to compare. CoreSight does not invent a baseline or show a zero change for missing data.",
  "history.empty.next":
    "Scan twice to see change. The first completed scan becomes the baseline; the next one produces the first comparison.",
  "history.change.title": "Since the previous snapshot",
  "history.change.descriptionFallback":
    "A comparison is not available yet. One valid baseline cannot answer what changed.",
  "history.change.total": "Total observed change",
  "history.change.range": "From {from} to {to}. {coverage}",
  "history.change.completeCoverage":
    "Both snapshots had comparable, complete coverage.",
  "history.change.partialCoverage":
    "At least one snapshot was partial; treat this as a lower-bound change.",
  "history.change.unknown": "Change unknown",
  "history.change.unknownBody":
    "A second completed snapshot is required. The baseline below is a fact, not a claim that nothing has changed since.",
  "history.delta.entryUnknown": "Entry change unknown",
  "history.delta.noEntryChange": "No entry change",
  "history.delta.entryChange.one": "{count} entry",
  "history.delta.entryChange.other": "{count} entries",
  "history.delta.aria":
    "{name}: {delta} since the previous snapshot. Open this category in Storage.",
  "history.delta.open": "Open in Storage ↗",
  "history.trigger.firstRun": "First baseline",
  "history.trigger.scheduled": "Scheduled snapshot",
  "history.trigger.manual": "Manual snapshot",
  "history.snapshot.observedTotal": "Observed total",
  "history.snapshot.changeVsPrevious": "Change vs previous",
  "history.snapshot.noEarlier": "No earlier snapshot",
  "history.snapshot.entries": "Entries",
  "history.timeline.title": "Snapshot timeline",
  "history.timeline.description":
    "Newest first. Each total carries its coverage so a partial scan cannot read as complete.",

  // ---------------------------------------------------------------------
  // Drives
  // ---------------------------------------------------------------------
  "drives.eyebrow": "Drives",
  "drives.title": "What drives do I have?",
  "drives.description":
    "Local and previously seen external drives, each with its connection state and last-known health.",
  "drives.stamp.title": "Drive memory",
  "drives.stamp.body": "Stored on this device",
  "drives.stamp.note": "Offline records stay read-only",
  "drives.empty.title": "No drives recorded yet",
  "drives.empty.body":
    "CoreSight has no connected or remembered drive records. This is an empty inventory, not a zero-byte drive.",
  "drives.empty.next":
    "After the first completed scan, the drive and its capacity context will appear here.",
  "drives.overview.title": "Drive inventory",
  "drives.overview.body":
    "Connected and remembered records share one list; their state is always labelled.",
  "drives.overview.connected": "Connected",
  "drives.overview.remembered": "Remembered",
  "drives.overview.writable": "Writable targets",
  "drives.overview.unavailablePartial": "Unavailable on partial data",
  "drives.list.title": "Drives",
  "drives.list.description":
    "Capacity is logical observed storage. Free space can be unknown; it is never filled in with a guess.",
  "drives.row.usedSpace": "Used space",
  "drives.row.usedSpaceLabel": "{name} used space",
  "drives.row.free": "Free",
  "drives.row.used": "Used",
  "drives.row.capacity": "Capacity",
  "drives.row.lastScan": "Last scan",
  "drives.row.snapshots": "Snapshots",
  "drives.row.snapshotsUnknown": "Snapshot count unknown",
  "drives.row.rememberedNote":
    "Last-known figures come from this drive's previous scan. A disconnected drive is read-only and cannot be a cleanup target.",
  "drives.memory.title": "How drive memory works",
  "drives.memory.description":
    "Remembered drives are records, not live mounts.",
  "drives.memory.connectedTerm": "Connected",
  "drives.memory.connectedBody":
    "means CoreSight can currently read the drive. Sizes and health are from the latest scan.",
  "drives.memory.rememberedTerm": "Remembered",
  "drives.memory.rememberedBody":
    "means the drive was seen before and is detached now. Its figures are explicitly last-known and cannot be acted on.",
  "drives.memory.unknownTerm": "Unknown remains unknown.",
  "drives.memory.unknownBody":
    "A disconnected drive's free space is not shown as zero, and stale health is never labelled healthy."
} as const;

export type MessageKey = keyof typeof en;

export const de: Record<MessageKey, string> = {
  // App shell
  "app.skipToContent": "Zum Inhalt springen",
  "app.brand.home": "CoreSight-Startseite",
  "app.nav.label": "Hauptnavigation",
  "app.nav.home": "Start",
  "app.nav.storage": "Speicher",
  "app.nav.cleanup": "Bereinigung",
  "app.nav.history": "Verlauf",
  "app.nav.drives": "Laufwerke",
  "app.nav.settings": "Einstellungen",
  "app.header.engineLive": "Lokale Engine",
  "app.header.enginePreview": "Lokale Vorschau",
  "app.header.engineLiveTitle":
    "Die lokale CoreSight-Engine wird über den coresight.v1-Vertrag gelesen.",
  "app.header.enginePreviewTitle":
    "Deterministische lokale Testdaten werden angezeigt; es ist keine Engine verbunden.",
  "app.footer.privacy":
    "CoreSight analysiert den Speicher lokal. Keine Pfade, Namen oder Hashes verlassen dieses Gerät.",
  "app.footer.build": "Phase-8-Oberfläche · Vertrag {contract} · {mode}",
  "app.footer.modeLive": "lokale Engine",
  "app.footer.modePreview": "deterministische Testdaten",

  // Contract mismatch
  "contract.eyebrow": "Vertragsprüfung",
  "contract.title": "Diese Oberfläche kann den installierten Datenvertrag nicht lesen",
  "contract.bodyBefore": "Der lokal zurückgegebene Vertrag ist ",
  "contract.bodyBetween": ", während diese Oberfläche ",
  "contract.bodyAfter":
    " unterstützt. CoreSight rät nicht bei unvereinbaren Feldern.",
  "contract.panelTitle": "Es wurden keine Bildschirme gefüllt",
  "contract.panelBody":
    "Aktualisiere die App, damit Frontend und Engine eine kompatible Vertragsversion teilen. Es wurden keine Daten geändert und kein Bereinigungspfad geöffnet.",

  // Ambient scan status
  "scan.label": "Scan-Status",
  "scan.loading": "Scan-Status wird gelesen…",
  "scan.unavailable": "Scan-Status nicht verfügbar",
  "scan.idle": "Noch kein abgeschlossener Scan",
  "scan.scanning": "Scan läuft · {seen} gesehen",
  "scan.scanningInArea": "Scan läuft · {seen} gesehen · {area}",
  "scan.failed": "Letzter Scan fehlgeschlagen · Zahlen können veraltet sein",
  "scan.partial":
    "Letzter Scan mit Einschränkungen abgeschlossen · {count} konnten nicht gelesen werden",
  "scan.complete": "Scan abgeschlossen · {time}",
  "scan.advanced": "{bytes} in diesem Scan gesehen · Kennung {id}",
  "scan.advancedBytesUnknown": "Byte-Zahl nicht gemeldet · Kennung {id}",

  // Shared state components
  "state.unknown": "Unbekannt",
  "state.partialTag.label": "teilweise",
  "state.partialTag.title":
    "Diese Zahl stammt aus einer teilweisen Quelle; sie ist keine vollständige Summe.",
  "state.coverage.incomplete.scan": "Dieser Scan ist unvollständig",
  "state.coverage.incomplete.storageScan": "Dieser Speicher-Scan ist unvollständig",
  "state.coverage.incomplete.cleanupPreview": "Diese Bereinigungsvorschau ist unvollständig",
  "state.coverage.incomplete.duplicateAnalysis": "Diese Duplikatanalyse ist unvollständig",
  "state.coverage.incomplete.largeFileResult":
    "Dieses Ergebnis der Großdateisuche ist unvollständig",
  "state.coverage.outsideHeading": "Einige Einträge liegen außerhalb dieses Ergebnisses",
  "state.coverage.incompleteBody":
    "Die Summen hier sind Untergrenzen, keine vollständigen Summen. Aggregierte Zahlen aus dieser Quelle tragen die Kennzeichnung „teilweise“.",
  "state.coverage.outsideBody":
    "Die Zahlen unten beschreiben nur die Einträge, die die Engine bewerten konnte.",
  "state.coverage.unreadableLabel": "Nicht lesbare Einträge:",
  "state.coverage.unknownSizesLabel": "Unbekannte Größen:",
  "state.coverage.unreadableAria": "Nicht lesbare Einträge: {value}",
  "state.coverage.unknownSizesAria": "Unbekannte Größen: {value}",
  "state.coverage.notIncluded": "Nicht enthalten:",
  "state.loading.heading": "Die lokale Momentaufnahme wird gelesen",
  "state.loading.body":
    "Große Scans können einen Moment dauern. Dieser Bildschirm aktualisiert sich, sobald die Daten bereit sind.",
  "state.loading.label": "Wird geladen",
  "state.error.eyebrow": "Dieser Bildschirm konnte nicht geladen werden",
  "state.error.nothingChanged":
    "Es wurde nichts geändert. Verlasse diesen Bildschirm und öffne ihn erneut; ist die Momentaufnahme weiterhin nicht verfügbar, starte CoreSight neu.",
  "state.error.returnHome": "Zurück zu Start",
  "state.error.reference": "Referenz",
  "state.sizeBar.unknown": "{label}: Anteil unbekannt",
  "state.sizeBar.share": "{label}: {percent}",
  "state.sizeBar.partialSuffix": ", aus einer teilweisen Quelle",

  // Safety tiers
  "tier.safe": "Sicher",
  "tier.review": "Prüfen",
  // NOTE: "Off-limits" has no single German adjective that keeps the safety
  // meaning; "Ausgeschlossen" (excluded) states what actually happens.
  "tier.offLimits": "Ausgeschlossen",

  // Counted units
  "unit.entry.one": "{count} Eintrag",
  "unit.entry.other": "{count} Einträge",
  "unit.opportunity.one": "{count} Möglichkeit",
  "unit.opportunity.other": "{count} Möglichkeiten",
  "unit.previewLine.one": "{count} Vorschauzeile",
  "unit.previewLine.other": "{count} Vorschauzeilen",
  "unit.excludedCategory.one": "{count} ausgeschlossene Kategorie",
  "unit.excludedCategory.other": "{count} ausgeschlossene Kategorien",

  // Cleanup
  "cleanup.eyebrow": "Bereinigung",
  "cleanup.title": "Was kann ich sicher entfernen?",
  "cleanup.description":
    "Sortierte Möglichkeiten mit ihrer Begründung. Eine Prüfung, eine Vorschau, eine ausdrückliche Bestätigung.",
  "cleanup.stamp.lastScan": "Letzter Scan",
  "cleanup.stamp.partial": "Teilweiser Scan · Bereinigungsaktionen gesperrt",
  "cleanup.stamp.actionsHere": "Aktionen nur in dieser Ansicht verfügbar",
  "cleanup.summary.safeEyebrow": "Sichere Schätzung",
  "cleanup.summary.safeBody":
    "Über Möglichkeiten der Stufe „Sicher“. Nichts wird automatisch verschoben.",
  "cleanup.summary.reviewEyebrow": "Prüfung nötig",
  "cleanup.summary.reviewBody":
    "Nur zur Prüfung gedachte Möglichkeiten, nie in einer sicheren Aktion gebündelt.",
  "cleanup.rules.title": "Es gibt keinen „Alles bereinigen“-Pfad.",
  "cleanup.rules.body":
    "Jede Möglichkeit hat ihre eigene Vorschau. Ausgeschlossene Kategorien bleiben unten sichtbar, mit dem Grund, warum sie nicht ausgewählt werden können.",
  "cleanup.blocked.title": "Aktionen sind deaktiviert, solange dieser Scan unvollständig ist.",
  "cleanup.blocked.body":
    "Sichere Pläne setzen vollständiges Wissen voraus. Die Möglichkeiten bleiben sichtbar, damit du prüfen kannst, was CoreSight gefunden hat, aber nichts kann zur Bestätigung vorgemerkt werden.",
  "cleanup.opportunities.title": "Möglichkeiten",
  "cleanup.opportunities.description":
    "Reihenfolge der Engine. Öffne einen Eintrag, um zu lesen, warum er vorgeschlagen wird, was unberührt bleibt und was als Nächstes passiert.",
  "cleanup.empty.noScanTitle": "Derzeit keine Bereinigungsmöglichkeiten",
  "cleanup.empty.noScanDescription":
    "Es gibt keinen abgeschlossenen Scan, der Bereinigungsmöglichkeiten erzeugen könnte. Das ist eine fehlende Datenlage, nicht die Aussage, dass nichts bereinigt werden kann.",
  "cleanup.empty.noScanNext":
    "Sobald ein vollständiger Scan abgeschlossen ist, erscheinen hier sichere und prüfbedürftige Möglichkeiten mit ihrer Begründung.",
  "cleanup.empty.noItemsDescription":
    "Der letzte abgeschlossene Scan hat nichts gefunden, was für eine sichere oder prüfbedürftige Bereinigung infrage kommt. Das ist ein leeres Ergebnis, keine verborgene Empfehlung.",
  "cleanup.empty.noItemsNext":
    "Behalte die Übersicht der ausgeschlossenen Kategorien unten als Kontext. Neue Möglichkeiten erscheinen nach dem nächsten abgeschlossenen Scan.",
  "cleanup.card.readDetails": "Details lesen",
  "cleanup.card.hideDetails": "Details ausblenden",
  "cleanup.card.readDetailsFor": "Details zu {title} lesen",
  "cleanup.card.hideDetailsFor": "Details zu {title} ausblenden",
  "cleanup.card.expectedRecovery": "Erwartete Freigabe",
  "cleanup.card.ageUnknown": "Alter unbekannt",
  "cleanup.card.oldestEvidence": "Ältester Beleg: {age}",
  "cleanup.card.recoveryUnknown": "Freigabemenge unbekannt",
  "cleanup.card.why": "Warum CoreSight das vorschlägt",
  "cleanup.card.stays": "Was unberührt bleibt",
  "cleanup.card.consequence": "Was nach dem Entfernen passiert",
  "cleanup.card.evidence": "Belege",
  "cleanup.card.previewMandatory": "Die Vorschau ist Pflicht.",
  "cleanup.card.previewMandatoryBody":
    "Der nächste Schritt zeigt genau, was verschoben würde, wohin es ginge und wie du es rückgängig machst. Keine Datei wird verschoben, bevor du bestätigst.",
  "cleanup.card.previewButton": "Vorschau dessen, was verschoben würde",
  "cleanup.offlimits.title": "Sichtbar ausgeschlossen",
  "cleanup.offlimits.description":
    "Ausgeschlossene Kategorien werden nicht verborgen. Sie werden gezeigt, damit die Grenze klar ist, ohne Aktionspfad.",
  "cleanup.offlimits.emptyNoScan":
    "Ohne abgeschlossenen Scan sind keine ausgeschlossenen Kategorien verfügbar.",
  "cleanup.offlimits.empty":
    "Für dieses Laufwerk wurden keine ausgeschlossenen Kategorien gemeldet.",
  "cleanup.offlimits.why": "Warum es ausgeschlossen ist:",
  "cleanup.offlimits.fallbackReason": "Durch die Engine-Richtlinie geschützt.",
  "cleanup.offlimits.sizeUnknown":
    "Größe unbekannt — diese Kategorie zählt nie als rückgewinnbarer Speicher.",
  "cleanup.offlimits.observed":
    "{size} beobachtet. Das ist Kontext, kein rückgewinnbarer Speicher.",
  "cleanup.blocking.scanIncomplete":
    "Die Bereinigung ist blockiert, weil der Scan unvollständig ist. Vor jeder Dateiverschiebung ist ein vollständiger Scan erforderlich.",
  "cleanup.blocking.noPreview":
    "Für diesen Eintrag gibt es keinen vollständigen Vorschaupfad; er kann nicht bestätigt werden.",
  "cleanup.blocking.unknownBytes":
    "Die rückgewinnbare Menge ist unbekannt, deshalb kann CoreSight keine vollständige Vorschau erstellen.",
  "cleanup.preview.eyebrow": "Genaue Vorschau · nichts wurde verschoben",
  "cleanup.preview.pageEyebrow": "Bereinigung · Vorschau",
  "cleanup.preview.title": "Prüfen: {title}",
  "cleanup.preview.description":
    "Schritt zwei von zwei. Dies ist die genaue Erklärung, die du bestätigst; noch ist nichts verschoben.",
  "cleanup.preview.back": "Zurück zu den Möglichkeiten",
  "cleanup.preview.building": "Die genaue Vorschau wird erstellt",
  "cleanup.preview.buildingBody":
    "Ausgewählte Einträge, Ziel, Rückgängig-Weg und Byte-Abrechnung werden geprüft.",
  "cleanup.preview.wouldMove": "Würde verschoben",
  "cleanup.preview.heading": "Genau das würde verschoben",
  "cleanup.preview.where": "Wohin es geht",
  "cleanup.preview.undo": "So machst du es rückgängig",
  "cleanup.preview.stays": "Was unberührt bleibt",
  "cleanup.preview.destinationUnknown": "Ziel unbekannt — die Bestätigung ist blockiert.",
  "cleanup.preview.undoUnknown":
    "Rückgängig-Weg unbekannt — die Bestätigung ist blockiert.",
  "cleanup.preview.warnings": "Warnungen, die du vor der Bestätigung lesen solltest",
  "cleanup.preview.cannotConfirm": "Diese Vorschau kann nicht bestätigt werden.",
  "cleanup.preview.cannotConfirmBody":
    "Ziel, Rückgängig-Weg, Byte-Menge oder ein vollständiger Scan fehlen. CoreSight blockiert den Plan, statt eine teilweise Bestätigung anzubieten.",
  "cleanup.confirm.acknowledgementAria":
    "Ich habe genau gelesen, was verschoben würde, wohin es ginge und wie ich es rückgängig mache.",
  "cleanup.confirm.acknowledgement":
    "Ich habe genau gelesen, was verschoben würde, wohin es ginge und wie ich es rückgängig mache. Mir ist klar, dass der Papierkorb diese Einträge nur schützt, bis er geleert wird.",
  // NOTE: "Trash" is the Windows Recycle Bin; German Windows calls it "Papierkorb".
  "cleanup.confirm.button": "Verschieben in den Papierkorb bestätigen",
  "cleanup.confirm.working": "Wird bestätigt…",
  "cleanup.confirm.microcopy":
    "Eine Möglichkeit nach der anderen. Für keinen anderen Bereinigungseintrag wird etwas unternommen.",
  "cleanup.confirm.liveConfirming": "Bereinigung wird bestätigt",
  "cleanup.confirm.liveChecked": "Bestätigung gesetzt",
  "cleanup.result.eyebrow": "Bereinigung · Bestätigung",
  "cleanup.result.panelEyebrow": "Ergebnis",
  "cleanup.result.title": "Bestätigung erfasst",
  "cleanup.result.description":
    "Die Bereinigung für „{title}“ hat die Vorschauprüfung passiert.",
  "cleanup.result.simulated": "UI-Vorschau im Arbeitsspeicher abgeschlossen",
  "cleanup.result.accepted": "Bereinigungsanfrage angenommen",
  "cleanup.result.freedBytes": "Freigegebene Bytes",
  "cleanup.result.entriesConfirmed": "Bestätigte Einträge",
  "cleanup.result.verification": "Verifikation",
  "cleanup.result.leftovers": "Es wurden Reste gemeldet",
  "cleanup.result.microcopy":
    "Es wird kein endgültiges Löschen angeboten. In dieser Phase-8-Oberfläche berührt der Mock-Adapter das Dateisystem nicht; Phase 9 verdrahtet die autorisierte IPC-Aktion.",
  "cleanup.result.return": "Verbleibende Möglichkeiten prüfen",
  "cleanup.result.defaultItem": "ausgewählte Bereinigung",
  "cleanup.error.message": "Diese Aktion konnte nicht abgeschlossen werden.",
  "cleanup.error.detail":
    "Nichts wurde verschoben. Kehre zur Liste der Möglichkeiten zurück und versuche es erneut.",

  // Settings
  "settings.eyebrow": "Einstellungen",
  "settings.title": "Einstellungen",
  "settings.description":
    "Wie CoreSight auf diesem Gerät aussieht und sich verhält. Diese Einstellungen werden lokal gespeichert und ändern nie, was CoreSight tun darf.",
  "settings.language.title": "Sprache",
  "settings.language.description":
    "Nur die Texte der Oberfläche. Alle fest eingebauten Oberflächentexte dieser Version sind übersetzt; von der Engine gelieferte Bezeichnungen werden unverändert angezeigt.",
  "settings.language.label": "Sprache der Oberfläche",
  "settings.language.en": "English",
  "settings.language.de": "Deutsch",
  "settings.appearance.title": "Darstellung",
  "settings.appearance.description":
    "Farbschema, Dichte und Bewegung. Farbschema und Bewegung folgen dem Betriebssystem, bis du etwas anderes wählst.",
  "settings.theme.legend": "Farbschema",
  "settings.theme.system": "System",
  "settings.theme.light": "Hell",
  "settings.theme.dark": "Dunkel",
  "settings.density.legend": "Dichte",
  "settings.density.comfortable": "Komfortabel",
  "settings.density.compact": "Kompakt",
  "settings.motion.legend": "Bewegung",
  "settings.motion.system": "System folgen",
  "settings.motion.reduce": "Bewegung reduzieren",
  "settings.motion.allow": "Bewegung erlauben",
  "settings.motion.hint":
    "„Bewegung reduzieren“ schaltet nicht wesentliche Animationen ab. „Bewegung erlauben“ übergeht eine Reduce-Motion-Einstellung des Betriebssystems.",
  "settings.screens.title": "Bereiche",
  "settings.screens.description": "Wähle, welche Bereiche in der Navigation erscheinen.",
  "settings.screens.hiddenNote":
    "Das Ausblenden eines Bereichs ist eine Anzeigeeinstellung, keine Änderung der Fähigkeiten. Jede Aktion, die etwas verändert, bleibt in der Bereinigung, hinter einer Vorschau und einer ausdrücklichen Bestätigung; hier lässt sich keine davon ein- oder ausschalten.",
  "settings.screens.lastVisible": "Mindestens ein Bereich muss sichtbar bleiben.",
  "settings.details.title": "Details",
  "settings.details.showAdvanced": "Erweiterte Details anzeigen",
  "settings.details.showAdvancedHint":
    "Ergänzt diagnostische Angaben in der Scan-Leiste, etwa Byte-Zahlen und die Scan-Kennung.",
  "settings.about.title": "Über diese Einstellungen",
  "settings.about.description":
    "Einstellungen sind reiner Client-Zustand. Sie liegen in diesem Browserprofil und werden nie an die Engine oder an einen anderen Ort gesendet.",
  "settings.about.storageKey": "Speicher-Schlüssel",
  "settings.about.schemaVersion": "Schema-Version",
  "settings.about.locale": "Aktive Sprache",
  "settings.about.reset": "Standardeinstellungen wiederherstellen",
  "settings.about.resetHint":
    "Setzt nur diese Einstellungen zurück. Dateien, Scan-Daten, Momentaufnahmen und Verlauf bleiben unberührt.",
  "settings.about.storageUnavailable":
    "Dieser Browser hat das Speichern der Einstellungen abgelehnt. Änderungen gelten nur für diese Sitzung.",
  "settings.about.saved": "Einstellungen wurden auf diesem Gerät gespeichert.",

  // Formatting sentinels and relative time
  "format.unknownChange": "Unbekannte Änderung",
  "format.noChange": "Keine Änderung",
  "format.ageUnknown": "Alter unbekannt",
  "format.lessThanADay": "Weniger als ein Tag",
  "format.etaUnknown": "Verbleibende Zeit unbekannt",
  "format.etaSeconds": "{count} Sek. verbleibend",
  "format.etaMinutes": "{count} Min. verbleibend",
  "format.shareUnknown": "Anteil unbekannt",
  "format.shareOfParent": "{percent} des übergeordneten Bereichs",

  // Shared status words and counted units
  "status.complete": "Vollständig",
  "status.partial": "Teilweise",
  "status.partialComparison": "Teilweiser Vergleich",
  "status.comparableCoverage": "Vergleichbare Abdeckung",
  "status.baselineOnly": "Nur Ausgangswert",

  "drive.health.healthy": "In Ordnung",
  "drive.health.attention": "Handlungsbedarf",
  "drive.health.unknown": "Zustand unbekannt",
  "drive.connection.connected": "Verbunden",
  "drive.connection.remembered": "Bekannt",
  "drive.connection.rememberedDisconnected": "Bekannt · getrennt",
  "drive.kind.internal": "Internes Laufwerk",
  "drive.kind.external": "Externes Laufwerk",
  "drive.kind.startupVolume": "Startvolume",

  "unit.day.one": "{count} Tag",
  "unit.day.other": "{count} Tage",
  "unit.subcategory.one": "{count} Unterkategorie",
  "unit.subcategory.other": "{count} Unterkategorien",
  "unit.label.one": "{count} Bezeichnung",
  "unit.label.other": "{count} Bezeichnungen",
  "unit.observedEntry.one": "{count} beobachteter Eintrag",
  "unit.observedEntry.other": "{count} beobachtete Einträge",
  "unit.category.one": "{count} Kategorie",
  "unit.category.other": "{count} Kategorien",
  "unit.topLevelGroup.one": "{count} Hauptgruppe",
  "unit.topLevelGroup.other": "{count} Hauptgruppen",
  "unit.group.one": "{count} Gruppe",
  "unit.group.other": "{count} Gruppen",
  "unit.verifiedCopy.one": "{count} verifizierte Kopie",
  "unit.verifiedCopy.other": "{count} verifizierte Kopien",
  "unit.member.one": "{count} Mitglied",
  "unit.member.other": "{count} Mitglieder",
  "unit.snapshot.one": "{count} Momentaufnahme",
  "unit.snapshot.other": "{count} Momentaufnahmen",
  "unit.drive.one": "{count} Laufwerk",
  "unit.drive.other": "{count} Laufwerke",
  "unit.connectedDrive.one": "{count} verbundenes Laufwerk",
  "unit.connectedDrive.other": "{count} verbundene Laufwerke",
  "unit.match.one": "{count} Treffer",
  "unit.match.other": "{count} Treffer",
  "unit.opportunityToReview.one": "{count} Möglichkeit zur Prüfung",
  "unit.opportunityToReview.other": "{count} Möglichkeiten zur Prüfung",

  // Home
  "home.eyebrow": "Start",
  "home.title": "Wie steht es um deinen Speicher?",
  "home.description":
    "Eine ruhige Einordnung der letzten lokalen Momentaufnahme. Kontext zuerst, technische Details später.",
  "home.empty.description":
    "Eine ruhige Zusammenfassung der Laufwerke, die CoreSight sieht, mit jeder Zahl in ihrem Kontext.",
  "home.empty.title": "Noch keine Laufwerksübersicht",
  "home.empty.body":
    "CoreSight hat auf diesem Gerät noch keinen Speicher-Scan abgeschlossen. Im Hintergrund wird nichts gemessen.",
  "home.empty.next":
    "Sobald ein Scan abgeschlossen ist, zeigt diese Seite freien Speicher, die größten Kategorien und die Änderungen.",
  "home.stamp.lastScan": "Letzter abgeschlossener Scan",
  "home.stamp.partial": "Teilweiser Scan · Summen unvollständig",
  "home.stamp.localOnly": "Nur lokal · nie hochgeladen",
  "home.safeToReclaim": "Sicher rückgewinnbar",
  "home.reviewAmountUnknown": "Prüfmenge unbekannt",
  "home.reviewInOpportunities": "{bytes} in Möglichkeiten, die nur zur Prüfung dienen",
  "home.hero.countUnknown": "Anzahl der Möglichkeiten unbekannt",
  "home.basisComplete": "basierend auf dem Scan, abgeschlossen am {time}.",
  "home.basisPartial": "basierend auf dem teilweisen Scan, abgeschlossen am {time}.",
  "home.rule":
    "Nichts wird automatisch verschoben. Jede Bereinigung wird einzeln in einer Vorschau geprüft und bestätigt.",
  "home.cleanupBlocked":
    "Dieser Scan ist unvollständig; die Bereinigung ist blockiert, bis ein vollständiger Scan erfolgreich ist.",
  "home.cta.openCleanup": "Bereinigungsprüfung öffnen",
  "home.cta.reviewSafe": "{bytes} zur sicheren Entfernung prüfen",
  "home.driveHealth": "Laufwerkszustand",
  "home.drive.lastKnownSuffix": " · zuletzt bekannt",
  "home.drive.usedSpaceLabel": "{name}: belegter Speicher",
  "home.drive.free": "{bytes} frei",
  "home.drive.freeUnknownValue": "Freier Speicher unbekannt",
  "home.drive.usedOfTotal": "{used} belegt von {total}",
  "home.drive.freeUnknown":
    "Der freie Speicher ist unbekannt, solange dieses Laufwerk getrennt ist – nicht null.",
  "home.categories.title": "Größte Kategorien",
  "home.categories.description":
    "Menschliche Kategorien zuerst. Öffne „Speicher“ für Unterkategorien und Beispielspeicherorte.",
  "home.categories.explore": "Speicher ansehen",
  "home.category.ofUsedSpace": "{percent} des belegten Speichers",
  "home.category.shareLabel": "{name}: Anteil am belegten Speicher",
  "home.since.title": "Seit dem letzten Scan",
  "home.since.description":
    "Die vorherige Momentaufnahme konnte nicht verglichen werden; es wird keine Änderung behauptet.",
  "home.change.across": "{delta} über die sichtbaren Kategorien",
  "home.change.unknown": "Änderung unbekannt",
  "home.change.unknownBody":
    "Scanne zweimal, um einen Vergleich zu erhalten. Ein fehlender Vergleich ist nicht dasselbe wie „nichts hat sich geändert“.",
  "home.change.viewHistory": "Scan-Verlauf ansehen",
  "home.change.seeWhatChanged": "Änderungen ansehen",
  "home.caveat.aria": "Hinweis zur Momentaufnahme",
  "home.caveat.title": "So liest du diese Zahlen.",
  "home.caveat.body":
    "Größen sind beobachtete logische Bytes, kein Versprechen rückgewinnbaren Speichers. Ein Kategorien-Delta wird als teilweise gekennzeichnet, wenn eine der Momentaufnahmen unvollständig war. Unbekannte Werte bleiben unbekannt.",

  // Storage
  "storage.eyebrow": "Speicher",
  "storage.title": "Was belegt meinen Speicher?",
  "storage.description":
    "Menschliche Kategorien zuerst, technische Bezeichnungen eine bewusste Stufe tiefer.",
  "storage.descriptionMain":
    "Beginne mit menschlichen Kategorien. Klappe nur auf, wenn du die Belege darunter sehen willst.",
  "storage.empty.title": "Noch keine Speicherkategorien",
  "storage.empty.body":
    "Für dieses Gerät gibt es keine abgeschlossene Kategorieaufteilung. Null Kategorien ist ein echtes leeres Ergebnis, keine unbekannte Summe.",
  "storage.empty.next":
    "Nach einem abgeschlossenen Scan erscheinen hier Kategorien, verifizierte Duplikate und große Dateien.",
  "storage.summary.observedTotal": "Beobachtete Gesamtsumme",
  "storage.summary.bodyComplete":
    "Über {count}. Das ist kein freier Speicher, sondern das, was der Scan beobachten konnte.",
  "storage.summary.bodyPartial":
    "Über {count}. Das ist kein freier Speicher, sondern das, was der teilweise Scan beobachten konnte.",
  "storage.summary.categories": "Kategorien",
  "storage.summary.entriesWithoutSize": "Einträge ohne Größe",
  "storage.summary.duplicateEvidence": "Duplikatbelege",
  "storage.categories.title": "Kategorien",
  "storage.categories.description":
    "Nach beobachteter Größe sortiert. Rohe Speicherorte erscheinen nur in der tiefsten Aufklappstufe.",
  "storage.search.label": "Kategorien suchen",
  "storage.search.aria": "Speicherkategorien suchen",
  "storage.search.placeholder": "Spiele, Dokumente, Entwicklung…",
  "storage.search.help": "Suche nach dem Namen der menschlichen Kategorie, nicht nach einem Pfad.",
  "storage.search.status": "{matches} für „{query}“.",
  "storage.search.emptyTitle": "Keine passenden Kategorien",
  "storage.search.emptyBody":
    "Keine menschliche Kategorie enthält „{query}“. Diese Suche rät nicht bei Pfadfragmenten.",
  "storage.search.emptyNext":
    "Versuche einen breiteren Kategorienamen oder leere die Suche, um alle Kategorien zu sehen.",
  "storage.column.category": "Kategorie",
  "storage.column.shareOfTotal": "Anteil an der Summe",
  "storage.column.observed": "Beobachtet",
  "storage.column.share": "Anteil",
  "storage.column.change": "Änderung",
  "storage.category.summary":
    "{name}: {bytes} beobachtet, {percent} des übergeordneten Bereichs, {delta} seit dem letzten Scan{partial}. {children}.",
  "storage.category.shareLabel": "{name}: Anteil",
  "storage.category.noDeeperBreakdown": "Keine tiefere Aufteilung",
  "storage.category.noDeeperBody": "Es wurde keine tiefere Kategorieaufteilung erfasst.",
  "storage.category.entries": "Einträge",
  "storage.category.entriesWithoutSize": "Einträge ohne bekannte Größe",
  "storage.category.observationErrors": "Beobachtungsfehler",
  "storage.category.sampleLocations": "Beispielspeicherorte",
  "storage.category.sampleLocationsAria": "Beispielspeicherorte für {name}",
  "storage.category.sampleLocationsNote":
    "Bezeichnungen stammen von der Engine und werden unverändert angezeigt. CoreSight baut oder löst in der Oberfläche keine Pfade auf.",
  // EN: "SHA-256" is an algorithm name, and "Hash" / "Hardlink-Alias" are the
  // established German technical terms; translating them would obscure meaning.
  "storage.duplicates.title": "Verifizierte Duplikatgruppen",
  "storage.duplicates.description":
    "Nur SHA-256-Übereinstimmungen mit gleicher Größe gelten als Duplikate. Hardlink-Aliase werden angezeigt, nie als Einsparung gezählt.",
  "storage.duplicates.recoverable": "Rückgewinnbar, verifiziert",
  "storage.duplicates.empty": "Es wurden keine hash-verifizierten Duplikatgruppen gefunden.",
  "storage.duplicates.group": "Gruppe {index}",
  "storage.duplicates.groupHeading": "{count} mit je {bytes}",
  "storage.duplicates.recoverableValue": "Rückgewinnbar: {bytes}",
  "storage.duplicates.verified": "SHA-256 verifiziert",
  "storage.duplicates.hashLabel": "Hash",
  "storage.duplicates.showMembersAria":
    "Mitgliedsbezeichnungen für Duplikatgruppe {index} anzeigen",
  "storage.duplicates.showMembers": "Mitgliedsbezeichnungen anzeigen",
  "storage.duplicates.hardlinkAlias": " · Hardlink-Alias",
  "storage.duplicates.hardlinkNote":
    "Ein Hardlink-Alias ist ein weiterer Name für dasselbe Dateisystemobjekt. Sein Entfernen gibt keinen Speicher frei und trägt daher nichts zu den rückgewinnbaren Bytes bei.",
  "storage.duplicates.unverified.one":
    "{count} Datei hat keinen verifizierten Inhalts-Hash und wird bei keiner Duplikatfeststellung berücksichtigt.",
  "storage.duplicates.unverified.other":
    "{count} Dateien haben keinen verifizierten Inhalts-Hash und werden bei keiner Duplikatfeststellung berücksichtigt.",
  "storage.duplicates.unknownSize.one":
    "{count} Eintrag hat einen verifizierten Hash, aber keine beobachtete Größe; Größengleichheit ist nicht beweisbar, daher erfolgt keine Gruppierung.",
  "storage.duplicates.unknownSize.other":
    "{count} Einträge haben einen verifizierten Hash, aber keine beobachtete Größe; Größengleichheit ist nicht beweisbar, daher erfolgt keine Gruppierung.",
  "storage.bigFiles.title": "Große Dateien",
  "storage.bigFiles.description":
    "Dateien über {bytes}. Ein unbekanntes Alter wird nie als alt behandelt.",
  "storage.bigFiles.empty": "Keine Datei hat die aktuelle Größenschwelle überschritten.",
  "storage.bigFiles.categoryUnknown": "Kategorie unbekannt",
  "storage.bigFiles.age": "{age} alt",
  "storage.bigFiles.modifiedUnknown": "Änderungszeit unbekannt",
  "storage.bigFiles.modified": "Geändert: {time}",
  "storage.bigFiles.showLocationAria":
    "Speicherort-Bezeichnung für die Datei mit {bytes} anzeigen",
  "storage.bigFiles.showLocation": "Speicherort-Bezeichnung anzeigen",
  "storage.bigFiles.unknownAge.one":
    "{count} Eintrag hatte eine unbekannte Änderungszeit und wurde von altersbasierten Schlüssen ausgeschlossen.",
  "storage.bigFiles.unknownAge.other":
    "{count} Einträge hatten eine unbekannte Änderungszeit und wurden von altersbasierten Schlüssen ausgeschlossen.",

  // History
  "history.eyebrow": "Verlauf",
  "history.title": "Was hat sich geändert?",
  "history.description":
    "Momentaufnahmen beantworten die +80-GB-Frage mit datierten, vergleichbaren Belegen — nie mit einem geratenen Trend.",
  "history.stamp.title": "Lokal gespeicherte Momentaufnahmen",
  "history.stamp.note": "Lokale Datenbank · nichts hochgeladen",
  "history.empty.title": "Noch keine Momentaufnahmen",
  "history.empty.body":
    "Es gibt keinen Verlauf zum Vergleichen. CoreSight erfindet keinen Ausgangswert und zeigt bei fehlenden Daten keine Nulländerung.",
  "history.empty.next":
    "Scanne zweimal, um Änderungen zu sehen. Der erste abgeschlossene Scan wird zum Ausgangswert; der nächste erzeugt den ersten Vergleich.",
  "history.change.title": "Seit der vorherigen Momentaufnahme",
  "history.change.descriptionFallback":
    "Ein Vergleich ist noch nicht verfügbar. Ein gültiger Ausgangswert kann nicht beantworten, was sich geändert hat.",
  "history.change.total": "Beobachtete Gesamtänderung",
  "history.change.range": "Von {from} bis {to}. {coverage}",
  "history.change.completeCoverage":
    "Beide Momentaufnahmen hatten vergleichbare, vollständige Abdeckung.",
  "history.change.partialCoverage":
    "Mindestens eine Momentaufnahme war teilweise; behandle dies als Untergrenze der Änderung.",
  "history.change.unknown": "Änderung unbekannt",
  "history.change.unknownBody":
    "Ein zweiter abgeschlossener Scan ist erforderlich. Der Ausgangswert unten ist eine Tatsache, keine Aussage, dass sich seitdem nichts geändert hat.",
  "history.delta.entryUnknown": "Eintragsänderung unbekannt",
  "history.delta.noEntryChange": "Keine Eintragsänderung",
  "history.delta.entryChange.one": "{count} Eintrag",
  "history.delta.entryChange.other": "{count} Einträge",
  "history.delta.aria":
    "{name}: {delta} seit der vorherigen Momentaufnahme. Öffne diese Kategorie im Speicher.",
  "history.delta.open": "Im Speicher öffnen ↗",
  "history.trigger.firstRun": "Erster Ausgangswert",
  "history.trigger.scheduled": "Geplante Momentaufnahme",
  "history.trigger.manual": "Manuelle Momentaufnahme",
  "history.snapshot.observedTotal": "Beobachtete Gesamtsumme",
  "history.snapshot.changeVsPrevious": "Änderung gegenüber vorher",
  "history.snapshot.noEarlier": "Keine frühere Momentaufnahme",
  "history.snapshot.entries": "Einträge",
  "history.timeline.title": "Zeitleiste der Momentaufnahmen",
  "history.timeline.description":
    "Neueste zuerst. Jede Summe trägt ihre Abdeckung, damit ein teilweiser Scan nicht als vollständig gelesen wird.",

  // Drives
  "drives.eyebrow": "Laufwerke",
  "drives.title": "Welche Laufwerke habe ich?",
  "drives.description":
    "Lokale und zuvor gesehene externe Laufwerke, jeweils mit Verbindungsstatus und zuletzt bekanntem Zustand.",
  "drives.stamp.title": "Laufwerksgedächtnis",
  "drives.stamp.body": "Auf diesem Gerät gespeichert",
  "drives.stamp.note": "Offline-Einträge bleiben schreibgeschützt",
  "drives.empty.title": "Noch keine Laufwerke erfasst",
  "drives.empty.body":
    "CoreSight hat keine verbundenen oder bekannten Laufwerkeinträge. Das ist ein leeres Inventar, kein Laufwerk mit null Bytes.",
  "drives.empty.next":
    "Nach dem ersten abgeschlossenen Scan erscheinen hier das Laufwerk und sein Kapazitätskontext.",
  "drives.overview.title": "Laufwerksinventar",
  "drives.overview.body":
    "Verbundene und bekannte Einträge teilen sich eine Liste; ihr Zustand ist immer gekennzeichnet.",
  "drives.overview.connected": "Verbunden",
  "drives.overview.remembered": "Bekannt",
  "drives.overview.writable": "Beschreibbare Ziele",
  "drives.overview.unavailablePartial": "Bei teilweisen Daten nicht verfügbar",
  "drives.list.title": "Laufwerke",
  "drives.list.description":
    "Die Kapazität ist logisch beobachteter Speicher. Freier Speicher kann unbekannt sein; er wird nie durch eine Schätzung ersetzt.",
  "drives.row.usedSpace": "Belegter Speicher",
  "drives.row.usedSpaceLabel": "{name}: belegter Speicher",
  "drives.row.free": "Frei",
  "drives.row.used": "Belegt",
  "drives.row.capacity": "Kapazität",
  "drives.row.lastScan": "Letzter Scan",
  "drives.row.snapshots": "Momentaufnahmen",
  "drives.row.snapshotsUnknown": "Anzahl der Momentaufnahmen unbekannt",
  "drives.row.rememberedNote":
    "Die zuletzt bekannten Zahlen stammen aus dem vorherigen Scan dieses Laufwerks. Ein getrenntes Laufwerk ist schreibgeschützt und kann kein Bereinigungsziel sein.",
  "drives.memory.title": "Wie das Laufwerksgedächtnis funktioniert",
  "drives.memory.description": "Bekannte Laufwerke sind Einträge, keine aktiven Einbindungen.",
  "drives.memory.connectedTerm": "Verbunden",
  "drives.memory.connectedBody":
    "bedeutet, dass CoreSight das Laufwerk derzeit lesen kann. Größen und Zustand stammen vom letzten Scan.",
  "drives.memory.rememberedTerm": "Bekannt",
  "drives.memory.rememberedBody":
    "bedeutet, dass das Laufwerk zuvor gesehen wurde und jetzt getrennt ist. Seine Zahlen sind ausdrücklich zuletzt bekannt; auf diesem Laufwerk kann keine Aktion ausgeführt werden.",
  "drives.memory.unknownTerm": "Unbekannt bleibt unbekannt.",
  "drives.memory.unknownBody":
    "Der freie Speicher eines getrennten Laufwerks wird nicht als null angezeigt, und ein veralteter Zustand wird nie als „In Ordnung“ bezeichnet."
};

export const MESSAGES: Record<"en" | "de", Record<MessageKey, string>> = { en, de };
