# SpaceLens — Design Principles

Status: Phase 0. Governs all future UI work. The goal: look like a finished
commercial product, not a generated dashboard.

## Character

Calm, precise, premium, restrained. Native-feeling on each OS. The interface
should recede; the *answer* should stand out. Trust is the aesthetic:
every pixel should say "this tool will not harm your files."

## Hard avoid list

- Excessive cards, random gradients, glassmorphism soup, decorative animation.
- Giant context-free numbers ("1,248,992 FILES!") and fake statistics.
- Emoji-based interface; placeholder/lorem content anywhere in shipped UI.
- Generic SaaS-dashboard layout (stat cards row + charts grid + sidebar of everything).
- Cluttered navigation, unnecessary sidebars, more than one accent color doing work.
- Inconsistent type scale, spacing, or corner radii across screens.
- Clutter that doesn't improve comprehension. When in doubt, remove.

## Positive rules

1. **Typography carries hierarchy.** One family (system stack preferred per OS),
   a fixed scale (e.g. 12 / 14 / 16 / 20 / 28), tabular numerals for sizes.
   Weight and size — not color — signal importance.
2. **One accent, used sparingly.** A single restrained accent for primary actions
   and selection. Safety tiers get their own restrained semantics
   (safe = neutral/green-muted, review = amber-muted, off-limits = never actionable,
   shown greyed with reason). No rainbow treemaps as the default view.
3. **Density over decoration.** Lists and bars beat cards for storage data.
   A storage row: name, size bar, size, delta. That's it.
4. **Spacing is a system.** 4pt base grid, consistent paddings. Screens share
   the same content width and section rhythm so the app feels like one product.
5. **Motion means state change.** Progress, expand/collapse, scan shimmer only.
   Nothing bounces, floats, or fades for personality.
6. **Numbers always have context.** Every size pairs with share-of-parent and,
   where history exists, delta-since-last. "18.4 GB" alone is a failure;
   "18.4 GB · 9% of drive · +2.1 GB this week" is the bar.
7. **Plain language, technical precision one layer down.** "Temporary files",
   not "tmpfs cache inodes". The exact path is always reachable via Details.
8. **Fear-reducing visuals.** Cleanup items show what stays and what happens
   *before* the action button. Destructive actions use explicit two-step confirm
   with the consequence restated, not a generic "Are you sure?"
9. **Accessibility is not polish.** Keyboard navigation for the category tree,
   visible focus, sufficient contrast in both themes, no color-only encoding
   (safety tiers always pair color with label + icon shape).
10. **Empty, loading, and error states are designed.** Each explains what happened,
    what it means for the numbers shown, and the one next step.

## Visualization guidance

- Default to ranked bars/lists (most readable, most honest).
- Treemap/sunburst are *optional views* inside Storage, not the home screen —
  they impress in screenshots but rank poorly for decision-making in testing
  across competitors.
- Never animate a visualization in a way that implies precision the data lacks
  (e.g. no smooth-counting to a number still being computed).

## No full UI in Phase 0

These principles are the design contract. Mockups, tokens, and implementation
belong to Phase 8 (Professional frontend), informed by the architecture docs here.
