# SpaceLens — Product Thesis

Status: Phase 0 foundation document. Hypotheses to be validated with real users before Phase 1 scope is locked.

## Who SpaceLens is for

Primary: non-technical to semi-technical desktop owners (Windows first, then macOS/Linux)
who hit "disk full" and feel anxiety, not curiosity. They do not know what AppData,
Library, or inodes are, and they should never need to.

Secondary: developers, gamers, and creators whose drives fill with large, legible
categories (repos, models, games, media) and who want fast answers plus drill-down control.

Explicitly NOT for (v1): enterprise storage admins, server fleets, compliance auditing.
Those are different products with different buyers.

## What problem it solves

When storage runs out, the user asks one question: **"What can I safely remove?"**
Everything else — treemaps, folder trees, file counts — is a means to that answer.
Today the user gets raw exposure (paths, sizes) and is left to bridge the gap to a
safe decision alone. That gap is where fear, confusion, and abandoned cleanups live.

## Why it matters

- A full drive degrades real work: updates fail, apps crash, captures stop.
- The cost of a wrong deletion (photos, documents, credentials, boot files) dwarfs
  the value of the space recovered. Fear is rational.
- Built-in OS tools (Windows Storage settings, macOS storage management) give vague
  buckets ("Other", "System Data") with no explanation and no safe path to action.

## Why existing products aren't enough

Researched in Phase 0 (Sep 2026): DaisyDisk ($9.99, Mac-only, visualization + deletion,
no duplicate engine), WizTree (Windows-only, fastest scanner via MFT, donationware +
site licenses, exposes data rather than explaining it), TreeSize (Free / $25.20/yr /
$49.20/yr, powerful but admin-flavored and subscription-moving), WinDirStat (free OSS,
slow, unmaintained, XP-era UI), SpaceSniffer (free portable treemap, stagnant),
DiskBuddy ($19 one-time Mac-only, 8 views, offline privacy-first — the closest
philosophical match, but single-platform).

Common gaps across all of them:

1. **Nobody explains.** Every tool shows *where* bytes are. None answers *what it is,
   why it's there, what happens if I remove it*.
2. **Nobody is cross-platform with a one-time price.** Every polished tool is
   single-OS. Nobody owns "pay once, understand all your machines."
3. **Cleanup is either reckless or absent.** One-click cleaners encourage blind
   deletion; analyzers refuse to help with the decision at all.
4. **No memory.** Nobody answers "what changed?" — the +80 GB question. Snapshots,
   history, and forensics are essentially missing from the consumer tier.
5. **Fear is unaddressed.** No product visibly separates *safe / review / never-touch*
   with enforced safety architecture behind the labels.

## Why someone would pay

- One honest scare ("disk full before a deadline/trip/release") creates willingness
  to pay for a tool that feels safe and certain.
- One-time purchase removes the subscription insult for a utility used a few times
  a year. $9.99–$29 one-time is proven territory (DaisyDisk, DiskBuddy).
- Cross-platform households (Windows desktop + MacBook) currently pay twice or
  settle for two free tools. One license for all machines is a concrete reason
  to switch.

## What SpaceLens does differently

1. **Explain, don't expose.** Categories in human language first
   ("Games — 312 GB"), technical paths one layer down, always.
2. **Every recommendation carries its reasoning.** What it is, why it's suggested,
   how much is recovered, what stays untouched, what happens after removal.
3. **Safety as architecture, not copy.** Scan → Analyze → Recommend → Review →
   Plan → Safety-validate → Confirm → Quarantine/Trash → Verify. Reversible by
   default; system/boot/user-data boundaries enforced in the engine, not the UI.
4. **Forensics.** Snapshots over time; "why did I gain 80 GB?" answered with
   per-category deltas.
5. **Cross-platform from day one, one purchase.** Windows + macOS + Linux,
   one license, local-first, privacy-first (nothing leaves the machine).

## What SpaceLens deliberately does NOT do (v1)

- No "PC optimizer / speed booster" claims. No registry cleaners. No snake oil.
- No automatic deletion. No background cleaning daemons.
- No cloud upload of file trees, hashes, or names. Ever.
- No duplicate-finder-as-afterthought: if it ships, it is hash-verified or it
  does not ship.
- No enterprise fleet management, no server agents, no compliance reporting.
- No subscription requirement. If a subscription ever exists, it funds an
  optional service (e.g. cloud backup intel), never the core unlock.

## Strongest marketing promise (candidate)

> **"Your drive is full. We'll tell you why — and what you can safely remove."**

Runner-up: "Complex engine. Simple experience." (kept as internal philosophy;
customer-facing copy should promise the outcome, not the architecture.)

## Central hypothesis — verdict

> "Existing tools expose storage. SpaceLens should explain storage."

**Verdict: strong enough to build on, with two conditions.**
First, "explain" must cash out as *safe decisions*, not prettier charts — the
explanation has to end in a confident Yes/No per item. Second, it must be paired
with the cross-platform one-time-purchase wedge, because "explains storage" alone
is copyable; "explains storage on every machine you own for one price" is a moat
at this price tier. Validate both with landing-page + prototype tests before
committing Phase 2+ scope.
