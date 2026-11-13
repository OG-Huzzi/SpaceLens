/**
 * Screen registry for the cross-cutting suites.
 *
 * The `src/screens/` tree is being rebuilt in parallel, so the suites must not
 * depend on internal class names or on the wording of a heading. They depend
 * on three things only:
 *
 *  1. the exported screen component and its props (the contract every screen
 *     already honours);
 *  2. the *role* of a control (`button`, `link`, `textbox`, …);
 *  3. the accessible name a control exposes.
 *
 * This module is the single place that knows how to mount a screen, so a
 * future screen is added in one place and every suite picks it up.
 */

import { act, render, waitFor, type RenderResult } from "@testing-library/react";
import type { ReactElement } from "react";
import { expect } from "vitest";
import type { CoreSightApi } from "../api";
import { CleanupScreen } from "../screens/CleanupScreen";
import { DrivesScreen } from "../screens/DrivesScreen";
import { HistoryScreen } from "../screens/HistoryScreen";
import { HomeScreen } from "../screens/HomeScreen";
import { SettingsScreen } from "../screens/SettingsScreen";
import { StorageScreen } from "../screens/StorageScreen";
import { SettingsProvider } from "../settings/SettingsProvider";
import type { CountingApi } from "./api";

export interface ScreenCase {
  /** Stable id used in test names. */
  id: string;
  /** Human label used in test names. */
  label: string;
  /** Mount the screen with the given API. */
  render: (api: CoreSightApi) => ReactElement;
}

/**
 * The five pillar screens from `docs/UX_ARCHITECTURE.md`.
 *
 * Cleanup is in this list too: it is a pillar screen, it is simply the one
 * screen that is allowed to offer actions.
 */
export const PILLAR_SCREENS: readonly ScreenCase[] = [
  { id: "home", label: "Home", render: (api) => <HomeScreen api={api} /> },
  {
    id: "storage",
    label: "Storage",
    render: (api) => <StorageScreen api={api} focusCategoryId={null} />
  },
  { id: "cleanup", label: "Cleanup", render: (api) => <CleanupScreen api={api} /> },
  {
    id: "history",
    label: "History",
    render: (api) => <HistoryScreen api={api} onOpenCategory={() => undefined} />
  },
  { id: "drives", label: "Drives", render: (api) => <DrivesScreen api={api} /> }
];

/** Settings is a route beside the pillars, not one of them. */
export const SETTINGS_SCREEN: ScreenCase = {
  id: "settings",
  label: "Settings",
  render: () => (
    <SettingsProvider>
      <SettingsScreen />
    </SettingsProvider>
  )
};

/**
 * Wait until the screen has finished reading.
 *
 * "Finished" means two things: no API call is in flight, and no element
 * claims `aria-busy` any more. Both are structural, so this keeps working
 * when the wording of a loading state changes.
 */
export async function waitForScreenReady(api: CountingApi): Promise<void> {
  // The mock resolves without a timer, so a microtask flush is enough. The
  // loop lets a screen that chains two reads (screen data, then a panel)
  // settle without waiting for a polling interval.
  for (let attempt = 0; attempt < 20; attempt += 1) {
    await act(async () => {
      await Promise.resolve();
    });
    if (
      api.inFlight === 0 &&
      document.querySelector('[aria-busy="true"]') === null
    ) {
      break;
    }
  }
  await waitFor(() => {
    expect(api.inFlight).toBe(0);
    expect(document.querySelector('[aria-busy="true"]')).toBeNull();
  });
}

/** Mount a screen and wait until it has settled. */
export async function renderScreen(
  testCase: ScreenCase,
  api: CountingApi
): Promise<RenderResult> {
  const result = render(testCase.render(api));
  await waitForScreenReady(api);
  return result;
}
