# CoreSight Design System

Status: authoritative token vocabulary. Any component that hard-codes a colour,
spacing value, radius, font size, shadow or duration instead of using these
tokens is a defect.

## The six principles

1. **Every number shows its evidence.** A byte count, a percentage or a delta
   never appears alone. It carries its source, its coverage, or an explicit
   unknown marker. A number without provenance is decoration.
2. **Unknown is a first-class visual state.** Not a blank, not a zero, not a
   dash. Unknown gets its own treatment: muted text, dashed border, and the word
   for it. A user must be able to tell "empty" from "unread" at a glance.
3. **Colour is semantic, never decorative.** The strong colours are reserved for
   the three safety tiers and for errors. Everything else is neutral. If a
   screen is colourful, something needs attention.
4. **Density with air.** An 8-point spatial grid, generous line-height, one
   primary action per view. Information-dense does not mean cramped.
5. **Motion explains, never decorates.** Animation shows where a thing came
   from or that state changed. It never plays for ambience, and it is fully
   suppressible.
6. **Keyboard-first.** Everything reachable without a mouse, focus always
   visible, and a command palette for anything more than two levels deep.

## Colour tokens

All colours are CSS custom properties on `:root`, overridden under
`[data-theme="dark"]`. Components must not reference raw hex values.

| Token | Purpose |
|---|---|
| `--cs-bg` | Page background |
| `--cs-surface` | Card / panel background |
| `--cs-surface-raised` | Elevated surface (dialog, popover, command palette) |
| `--cs-surface-sunken` | Recessed area (code, table header, inset) |
| `--cs-border` | Default hairline |
| `--cs-border-strong` | Emphasised divider |
| `--cs-border-unknown` | Dashed border marking an unknown value |
| `--cs-text` | Primary text |
| `--cs-text-muted` | Secondary text, units, metadata |
| `--cs-text-subtle` | Tertiary text, disabled |
| `--cs-text-inverse` | Text on an accent fill |
| `--cs-accent` | Brand / interactive accent |
| `--cs-accent-hover` | Accent hover |
| `--cs-accent-subtle` | Accent tint background |
| `--cs-safe` | Safety tier: safe |
| `--cs-safe-subtle` | Safe tint background |
| `--cs-review` | Safety tier: review |
| `--cs-review-subtle` | Review tint background |
| `--cs-offlimits` | Safety tier: off-limits / destructive |
| `--cs-offlimits-subtle` | Off-limits tint background |
| `--cs-warning` | Attention, incomplete coverage |
| `--cs-warning-subtle` | Warning tint background |
| `--cs-danger` | Error |
| `--cs-danger-subtle` | Error tint background |
| `--cs-focus` | Focus ring |
| `--cs-viz-hatch` | The single shared "this value is not known" texture |

Dark mode is the primary theme, not an afterthought: the app is used to inspect
storage at night as often as in daylight. Both themes must meet WCAG AA for
body text.

## Colour in a proportional map

A map encodes area. Its colour must encode something else, or it is
decoration. There are exactly two permitted colour states in a map region:

- **Neutral** — the default. `--cs-surface-raised` with a hairline separator.
  A region with no safety classification carries no colour, because the app
  has no safety judgement about it.
- **A safety tier** — `--cs-safe`, `--cs-review`, `--cs-offlimits`, exactly
  as the tier badge renders it. The map and the legend must show the same
  colour for the same category.

There is no ramp. A lightness ramp indexed by rank would encode nothing a
reader can act on, and `docs/DESIGN_PRINCIPLES.md` is right that a rainbow
map "impresses in screenshots but ranks poorly for decision-making".

The unknown region is neither: it carries `--cs-viz-hatch` and a dashed
`--cs-border-unknown` border, so it reads as "not measured" from across the
room.

`--cs-viz-hatch` is the one hatch in the product. Every visualisation that
needs to show "unknown" uses it, so a reader learns the texture once and
recognises it everywhere.

## Unknown, rendered

`0 B` and "could not measure" are different claims and must never look alike:

- **A measured zero** is plain tabular digits in `--cs-text`, no border.
- **An unknown value** is the word "Unknown" in `--cs-text-subtle`, inside a
  `1px dashed` `--cs-border-unknown` box, set in `--cs-font-mono` at
  `--cs-text-xs`.
- **An unknown share** is a `--cs-viz-hatch` fill with a dashed border — never
  an empty bar, because an empty bar reads as "zero".


## Space — 8-point grid

`--cs-space-1` = 4px, `-2` = 8, `-3` = 12, `-4` = 16, `-5` = 24, `-6` = 32,
`-7` = 40, `-8` = 48, `-9` = 64, `-10` = 80, `-11` = 96, `-12` = 128.

Use only these steps. A component that needs 14px wants `-3` (12) or `-4` (16).

## Radius

`--cs-radius-sm` 6px · `--cs-radius-md` 10px · `--cs-radius-lg` 16px ·
`--cs-radius-full` 9999px.

## Typography

`--cs-font-sans` — UI · `--cs-font-mono` — every number, path, hash and byte
count. Numerals in mono keep columns aligned; a proportional digit in a size
column is a defect.

Sizes: `--cs-text-xs` 11px · `-sm` 12px · `-base` 14px · `-md` 16px ·
`-lg` 20px · `-xl` 26px · `-2xl` 34px · `-3xl` 44px.

Line heights: `--cs-leading-tight` 1.15 (display) · `--cs-leading-snug` 1.35
(headings) · `--cs-leading-normal` 1.55 (body).

Weights: `--cs-weight-regular` 400 · `-medium` 500 · `-semibold` 600.

Tabular numerals (`font-variant-numeric: tabular-nums`) on every data column.

## Elevation

`--cs-shadow-1` — resting card · `--cs-shadow-2` — raised · `--cs-shadow-3` —
dialog / palette. In dark mode elevation is expressed by surface lightness
first and shadow second; a shadow alone is invisible on a dark background.

## Motion

`--cs-duration-instant` 80ms · `-fast` 140ms · `-base` 220ms · `-slow` 360ms.
`--cs-ease-standard` cubic-bezier(.2,0,0,1) · `--cs-ease-emphasized`
cubic-bezier(.2,0,0,1.2).

Every transition must be wrapped so that
`@media (prefers-reduced-motion: reduce)` and `[data-reduced-motion="reduce"]`
collapse it to a near-instant opacity change. Motion that survives a
reduced-motion setting is a bug.

## Layout

`--cs-sidebar-width` 248px · `--cs-content-max` 1180px ·
`--cs-panel-pad` `var(--cs-space-5)`.

Also layout-scoped: `--cs-measure` 68ch (body measure) · `--cs-measure-wide`
88ch (wide explanatory copy) · `--cs-control-height` 40px ·
`--cs-row-height` 56px · `--cs-row-height-compact` 48px (the compact density
preference) · `--cs-header-offset` 148px (the height the sticky shell
occupies; column headers and anchor targets offset by it).

## Required primitive components

These live in `src/design/`. Names and props are fixed; screens import them
rather than re-implementing them.

| Component | Props |
|---|---|
| `<Surface>` | `tone?: "default" \| "raised" \| "sunken"`, `padded?: boolean` |
| `<Panel>` | `title`, `description?`, `actions?`, `children` |
| `<Metric>` | `label`, `value: ReactNode`, `unit?`, `unknown?: boolean`, `delta?`, `hint?` |
| `<Tier>` | `tier: "SAFE" \| "REVIEW" \| "OFF_LIMITS"`, `children?` |
| `<UnknownValue>` | `label?: string` — renders the unknown state, never a zero |
| `<CoverageBanner>` | `complete: boolean`, `erroredEntries?: number`, `entriesWithoutSize?: number`, `context` |
| `<EmptyState>` | `title`, `body?`, `next?` |
| `<Skeleton>` | `lines?: number`, `height?` |
| `<Button>` | `variant: "primary" \| "secondary" \| "ghost" \| "danger"`, `disabled?`, `onClick?` |
| `<KeyHint>` | `keys: string[]` |
| `<Field>` | `label`, `hint?`, `error?`, `children` |

## Anti-patterns that must not appear

- A coloured number with no label or unit.
- A "0 B" where the truth is "we could not measure".
- A progress indicator that animates without conveying real progress.
- A destructive control outside the Cleanup screen.
- A tooltip that hides a safety explanation. Safety copy is always visible.
- An icon-only button with no `aria-label`.
- Any colour that is not from this token set.
