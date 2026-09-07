# SpaceLens — UX Architecture

Status: Phase 0. Direction, not final mockups. No production UI is built in Phase 0.

## First principle

> **The user should not have to understand the filesystem.**

Every screen translates engine output into human categories first, with the raw
filesystem exactly one deliberate step away ("Details" / "Show paths"), never
the default view.

## Navigation (v1)

Five top-level destinations. Each owns exactly one user question.
No screen is added because a feature exists — each must answer its question
or be cut.

| Screen    | Answers                          | Core content |
|-----------|----------------------------------|--------------|
| **Home**    | "How is my storage doing?"       | Health summary per drive: used/free, trend vs last snapshot, top 3 categories, one primary action ("Review 12 GB you can safely remove"). No giant meaningless numbers: every figure carries context (what changed, since when). |
| **Storage** | "What's using my space?"         | Human categories (Applications, Games, Photos & Videos, Documents, Downloads, Media, Development, System, Temporary), each expandable to subcategories, then to paths at the deepest layer. Search within. |
| **Cleanup** | "What can I safely remove?"      | Ranked opportunities. Each card: what it is, why recommended, size, expected recovery, what stays untouched, consequence of removal. Safety tier visible: Safe / Review / Off-limits (off-limits never listed as actionable). Preview-before-action mandatory. |
| **History** | "What changed?"                  | Snapshot timeline: total + per-category deltas ("+31 GB Docker, +18 GB Downloads in 7 days"). Selecting a delta jumps to the category in Storage. |
| **Drives**  | "What drives do I have?"         | Local + previously-seen external drives (drive memory), per-drive health, last scan time, rescan. External drive state clearly marked (connected / remembered). |

## Cross-cutting rules

- **One purpose per screen.** If a widget doesn't serve the screen's question, it moves or dies.
- **Progressive disclosure, always.** Category → subcategory → folder → file path.
  Raw paths never appear above the lowest layer except inside an explicit Details view.
- **Actions live in Cleanup only.** Storage explains; History remembers; only Cleanup
  proposes removal — and every proposal ends in preview + confirm + reversible action.
- **Scanning is ambient, never blocking.** Progress is a quiet indicator with
  cancel; partial results stream in; the UI never beachballs on a million files.
- **Empty/error states teach.** "No snapshots yet — scan twice to see change" beats
  a blank chart. Permission-denied areas are labeled as unscanned, never silently
  dropped from totals (totals say what they exclude).
- **No dark patterns.** No scary red "87 ISSUES FOUND" counters, no one-click
  "Clean everything" button, no fake urgency.

## Deliberately excluded (v1)

Settings labyrinth (a handful of preferences max), themes/skins marketplace,
social/sharing features, menu-bar-widget sprawl. The app is five screens done well.
