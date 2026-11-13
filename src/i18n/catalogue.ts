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
    "Interface strings only. The rest of this build is still being translated.",
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
  "settings.about.saved": "Preferences saved on this device."
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
    "Nur die Texte der Oberfläche. Der Rest dieser Version wird noch übersetzt.",
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
  "settings.about.saved": "Einstellungen wurden auf diesem Gerät gespeichert."
};

export const MESSAGES: Record<"en" | "de", Record<MessageKey, string>> = { en, de };
