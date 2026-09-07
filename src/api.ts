/**
 * Typed mirror of `spacelens.v1.*` (docs/API_CONTRACTS.md, crates/spacelens-core/src/contract.rs).
 * The UI never handles filesystem paths — only categories and opportunities.
 */

export const CONTRACT_VERSION = "v1" as const;

export interface Category {
  id: string;
  name: string;
  bytes: number;
  shareOfParent: number;
  deltaSinceLast: number | null;
  itemCount: number;
}

export type SafetyTier = "safe" | "review";

export interface Opportunity {
  id: string;
  tier: SafetyTier;
  title: string;
  whatItIs: string;
  whyRecommended: string;
  bytes: number;
  recoverableBytes: number;
  staysUntouched: string;
  consequence: string;
}

export interface ApiError {
  code: string;
  message: string;
  detail?: string;
}

/** Phase 0 stub: validates the type surface until Tauri invoke is wired in Phase 1. */
export function isContractCompatible(version: string): boolean {
  return version === CONTRACT_VERSION;
}
