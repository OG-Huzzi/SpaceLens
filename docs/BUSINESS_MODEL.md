# SpaceLens — Business Model

Status: Phase 0 analysis. No payment or licensing code is built in Phase 0.

## Target customer

- Primary: home users and prosumers on 1–3 machines who will pay once to resolve
  storage pain safely. Proven willingness exists at $9.99 (DaisyDisk) and $19
  (DiskBuddy), both Mac-only.
- Secondary: Windows-first gamers/developers/creators with large, fast-growing
  drives. Underserved: the polished one-time tools are all Mac-only; the Windows
  side is free-but-raw (WizTree) or subscription-moving (TreeSize).
- Not targeting in v1: enterprise/IT (site licenses, audits), which is TreeSize
  Professional's home turf.

## Competitive pricing (researched Sep 2026)

| Product | Model | Price signal |
|---|---|---|
| DaisyDisk | One-time | $9.99, Mac-only |
| DiskBuddy | One-time | $19, Mac-only |
| WizTree | Free + commercial licenses | Free personal; site licenses ~$100–$1,800 (1 yr updates, perpetual use) |
| TreeSize | Freemium → subscription | Free; Personal ~$25.20/user/yr; Professional ~$49.20/user/yr |
| WinDirStat / SpaceSniffer | Free OSS/donationware | $0, stagnant |

Takeaway: the **cross-platform one-time purchase** slot is empty. Everything
polished is single-OS; everything cross-platform is free-and-raw or drifting
to subscription.

## Recommended structure: Free + One-time Pro

- **Free:** full scanner + Storage + Drives + one snapshot. Enough to deliver
  the "oh, THAT's where it went" moment. No time limit, no ads, no nagware.
  The free tier is the marketing.
- **Pro (one-time, per-user, all platforms):** Cleanup engine with explanations +
  preview + quarantine, duplicate detection, History/forensics, drive memory,
  unlimited snapshots. One license covers all of a user's machines
  (Windows + macOS + Linux).
- **Price positioning:** $24–$29 launch anchor. Above DiskBuddy/DaisyDisk
  (justified: 3 platforms + cleanup + forensics), far below TreeSize Pro's
  recurring cost. Run introductory pricing, never fake discounts or countdowns.

## Why someone pays

1. The free scan finds the pain; Pro resolves it safely (explanations + preview
   + reversible action). The conversion moment is concrete: "remove these 12 GB
   with confidence."
2. One license for every machine they own — nobody else offers this at this tier.
3. No subscription for a utility used episodically. This is a values purchase
   as much as a feature purchase; honor it and it becomes word-of-mouth.

## Risks

- Free tools are "good enough" for technical users (WizTree + discipline).
  Mitigation: sell safety + explanation to non-technical users, not speed to experts.
- One-time revenue caps lifetime value; major-version upgrades must be genuinely
  major to re-monetize. Mitigation: keep team small, price honestly, consider an
  *optional* paid service later (never ransom core features).
- App-store and OS-vendor risk: Apple/Microsoft sherlocking, store commission,
  notarization/sandbox limits on filesystem access. Mitigation: direct sale as
  primary channel; stores as secondary.
- **Brand collision (documented, not resolved):** "SpaceLens"/"Spacelens" marks
  exist in adjacent spaces — spacelens.com (e-commerce/blockchain), an iOS
  "SpaceLens: Storage Cleaner Pro", and notably an npm/Qt disk-analysis tool
  also named spacelens. Legal clearance + possible qualifier (e.g. "SpaceLens
  Disk Intelligence") must precede paid launch. No legal claims made here.
- Single-OS users may compare only against their free native tool. Mitigation:
  free tier must beat WizTree/GrandPerspective on clarity, not just match on speed.

## Opportunities

- Cross-platform households (Win desktop + MacBook) have no single answer today.
- Forensics ("what changed?") has no consumer-tier owner — biggest differentiation
  per engineering cost after the explanation layer.
- Privacy-first + offline is increasingly marketable; competitors rarely lead with it.
- Education content (what is AppData/Library, what is safe) doubles as SEO moat.

## What we will not do

Promise revenue. Claim guaranteed success. Use subscriptions-by-stealth,
feature-ransoming, or dark-pattern upsells. Ship licensing/payment code before
the product earns trust (Phase 11+ concern, not Phase 0/1).
