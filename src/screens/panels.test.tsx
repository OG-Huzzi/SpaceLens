import { render, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { describe, expect, it } from "vitest";
import { createMockApi, type ApiError, type CoreSightApi } from "../api";
import { BlockListPanel } from "./BlockListPanel";
import { DnsRankingPanel } from "./DnsRankingPanel";
import { HistoryScreen } from "./HistoryScreen";
import { HomeScreen } from "./HomeScreen";
import { PrivacyPanel } from "./PrivacyPanel";
import { StartupPanel } from "./StartupPanel";

function makeApi(): CoreSightApi {
  return createMockApi({ latencyMs: 0 });
}

function failingApi(
  method: keyof CoreSightApi,
  code: "SESSION_UNAVAILABLE" | "HISTORY_UNAVAILABLE" = "SESSION_UNAVAILABLE"
): CoreSightApi {
  const api = makeApi();
  const error: ApiError = {
    code,
    message: "The session has no inventory for this record.",
    detail: "No local inventory was registered for this build. Nothing was read and nothing was changed."
  };
  Object.assign(api, { [method]: () => Promise.reject(error) });
  return api;
}

async function expectUnavailable(panel: ReactElement, emptyMarker: RegExp) {
  render(panel);
  expect(
    await screen.findByRole("heading", { name: "Not available in this build" })
  ).toBeInTheDocument();
  expect(
    screen.getByText(/The session has no inventory for this record/)
  ).toBeInTheDocument();
  expect(screen.getByText(/No local inventory was registered/)).toBeInTheDocument();
  expect(screen.queryByText(emptyMarker)).not.toBeInTheDocument();
  expect(screen.queryByText("0 B")).not.toBeInTheDocument();
}

describe("StartupPanel", () => {
  it("renders the inventory, its reasons, and separates off-limits items", async () => {
    render(<StartupPanel api={makeApi()} />);

    await screen.findByRole("heading", { name: "Startup inventory" });
    expect(screen.getByText("This build cannot disable anything.")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Archive Backup Helper" })
    ).toBeInTheDocument();
    expect(screen.getByText("Source: User login item")).toBeInTheDocument();
    expect(screen.getByText("Safe to disable")).toBeInTheDocument();
    expect(screen.getByText(/Disabling it stops automatic backups/)).toBeInTheDocument();

    const offLimits = screen.getByRole("region", { name: "Off-limits startup items" });
    expect(within(offLimits).getByText("Security agent")).toBeInTheDocument();
    expect(within(offLimits).getByText(/Privileged system source/)).toBeInTheDocument();
    expect(
      within(offLimits).getByText(/Why it is off-limits:/)
    ).toBeInTheDocument();
    expect(within(offLimits).queryAllByRole("button")).toHaveLength(0);
  });

  it("renders the calm unavailable state for SESSION_UNAVAILABLE", async () => {
    await expectUnavailable(
      <StartupPanel api={failingApi("getStartupReport")} />,
      /No startup items were reported/
    );
    expect(screen.queryByText("Enabled items reported")).not.toBeInTheDocument();
  });

  it("renders the calm unavailable state for HISTORY_UNAVAILABLE", async () => {
    render(
      <StartupPanel api={failingApi("getStartupReport", "HISTORY_UNAVAILABLE")} />
    );
    expect(
      await screen.findByRole("heading", { name: "Not available in this build" })
    ).toBeInTheDocument();
    expect(screen.getByText("HISTORY_UNAVAILABLE")).toBeInTheDocument();
  });
});

describe("DnsRankingPanel", () => {
  it("renders component scores and every excluded candidate with its reason", async () => {
    render(<DnsRankingPanel api={makeApi()} />);

    await screen.findByRole("heading", { name: "DNS resolver ranking" });
    expect(screen.getByText("Resolver A")).toBeInTheDocument();
    expect(screen.getByText("Resolver B")).toBeInTheDocument();
    expect(screen.getAllByText("Latency score").length).toBeGreaterThan(0);
    expect(screen.getAllByText("88 / 100").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText(/Counts for 40% of the total score/).length
    ).toBeGreaterThan(0);
    expect(screen.getByText("CoreSight never switches DNS.")).toBeInTheDocument();
    expect(
      screen.getByText(/Changing DNS changes who can see every lookup/)
    ).toBeInTheDocument();

    const excluded = screen.getByRole("region", { name: "Excluded candidates" });
    expect(within(excluded).getByText("Unmeasured resolver")).toBeInTheDocument();
    expect(within(excluded).getByText("Not measured")).toBeInTheDocument();
    expect(within(excluded).getByText("NOT_MEASURED")).toBeInTheDocument();
    expect(within(excluded).getByText(/No benchmark samples/)).toBeInTheDocument();
  });

  it("renders the calm unavailable state instead of an empty ranking", async () => {
    await expectUnavailable(
      <DnsRankingPanel api={failingApi("getDnsRanking")} />,
      /No resolver measurements/
    );
    expect(screen.queryByText("Ranked candidates")).not.toBeInTheDocument();
  });
});

describe("BlockListPanel", () => {
  it("groups entries by category and surfaces rejection, duplicate, and whitelist counts", async () => {
    render(<BlockListPanel api={makeApi()} />);

    await screen.findByRole("heading", { name: "Block-list plan" });
    expect(
      screen.getByText("This is a plan. Nothing was written to the hosts file.")
    ).toBeInTheDocument();
    expect(screen.getByText("Advertising")).toBeInTheDocument();
    expect(screen.getByText("ads.example.test")).toBeInTheDocument();
    expect(screen.getByText("Tracking")).toBeInTheDocument();
    expect(screen.getByText("tracker.example.test")).toBeInTheDocument();

    expect(screen.getByText("Duplicates removed")).toBeInTheDocument();
    expect(screen.getByText("1 duplicate removed")).toBeInTheDocument();
    expect(screen.getAllByText("1 rejected entry").length).toBeGreaterThan(0);
    expect(screen.getByText("not a domain")).toBeInTheDocument();
    expect(screen.getByText("contains whitespace")).toBeInTheDocument();
    expect(screen.getAllByText("1 whitelisted domain").length).toBeGreaterThan(0);
    expect(screen.getByText("bank.example.test")).toBeInTheDocument();
  });

  it("renders the calm unavailable state instead of an empty plan", async () => {
    await expectUnavailable(
      <BlockListPanel api={failingApi("getBlockListPlan")} />,
      /No block-list plan/
    );
    expect(screen.queryByText("Planned entries by category")).not.toBeInTheDocument();
  });
});

describe("PrivacyPanel", () => {
  it("renders findings by kind and never turns an unknown total into 0 B", async () => {
    render(<PrivacyPanel api={makeApi()} />);

    await screen.findByRole("heading", { name: "Privacy findings" });
    expect(screen.getByText("Tracking cookies")).toBeInTheDocument();
    expect(screen.getByText("Session cookies")).toBeInTheDocument();
    expect(screen.getByText("Cache entries")).toBeInTheDocument();
    expect(screen.getByText("Estimated bytes")).toBeInTheDocument();
    expect(screen.getByText("Unknown")).toBeInTheDocument();
    expect(screen.queryByText("0 B")).not.toBeInTheDocument();
    expect(
      screen.getByText(/The total is unknown because at least one finding has no size estimate/)
    ).toBeInTheDocument();
  });

  it("keeps the session-cookie sign-out warning visible", async () => {
    render(<PrivacyPanel api={makeApi()} />);

    await screen.findByRole("heading", { name: "Privacy findings" });
    expect(screen.getByText("Session cookies keep you signed in.")).toBeInTheDocument();
    expect(
      screen.getByText(/Removing session cookies would sign you out of websites/)
    ).toBeInTheDocument();
  });

  it("renders the calm unavailable state instead of an empty summary", async () => {
    await expectUnavailable(
      <PrivacyPanel api={failingApi("getPrivacySummary")} />,
      /No privacy findings/
    );
    expect(screen.queryByText("Findings by kind")).not.toBeInTheDocument();
  });
});

describe("panel states", () => {
  it("renders an explicit empty state for every panel", async () => {
    const emptyApi = () => createMockApi({ latencyMs: 0, scenario: "empty" });
    const cases: Array<[ReactElement, string]> = [
      [<StartupPanel api={emptyApi()} />, "No startup items were reported"],
      [<DnsRankingPanel api={emptyApi()} />, "No resolver measurements"],
      [<BlockListPanel api={emptyApi()} />, "No block-list plan"],
      [<PrivacyPanel api={emptyApi()} />, "No privacy findings"]
    ];

    for (const [panel, expected] of cases) {
      const { unmount } = render(panel);
      expect(await screen.findByText(expected)).toBeInTheDocument();
      unmount();
    }
  });

  it("keeps a real read failure as an error rather than an unavailable record", async () => {
    const api = makeApi();
    const error: ApiError = {
      code: "SNAPSHOT_UNAVAILABLE",
      message: "CoreSight could not read the local snapshot.",
      detail: "Nothing was changed."
    };
    Object.assign(api, { getStartupReport: () => Promise.reject(error) });

    render(<StartupPanel api={api} />);

    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("Not available in this build")).not.toBeInTheDocument();
  });

  it("renders History as a calm unavailable record instead of a generic error", async () => {
    render(
      <HistoryScreen
        api={failingApi("getHistory", "HISTORY_UNAVAILABLE")}
        onOpenCategory={() => undefined}
      />
    );

    expect(
      await screen.findByRole("heading", { name: "Not available in this build" })
    ).toBeInTheDocument();
    expect(screen.getByText("HISTORY_UNAVAILABLE")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Startup inventory" })
    ).toBeInTheDocument();
  });

  it("keeps the privacy panel reachable when the home summary is unavailable", async () => {
    render(<HomeScreen api={failingApi("getHomeSummary")} />);

    expect(
      await screen.findByRole("heading", { name: "Not available in this build" })
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Privacy findings" })
    ).toBeInTheDocument();
  });
});

describe("read-only guarantee", () => {
  it("adds no state-changing control to any panel", async () => {
    const panels: Array<[string, ReactElement, string]> = [
      ["startup", <StartupPanel api={makeApi()} />, "Startup inventory"],
      ["dns", <DnsRankingPanel api={makeApi()} />, "DNS resolver ranking"],
      ["block-list", <BlockListPanel api={makeApi()} />, "Block-list plan"],
      ["privacy", <PrivacyPanel api={makeApi()} />, "Privacy findings"]
    ];

    for (const [name, panel, heading] of panels) {
      const { container, unmount } = render(panel);
      await screen.findByRole("heading", { name: heading });

      expect(within(container).queryAllByRole("button"), `${name} buttons`).toHaveLength(0);
      expect(within(container).queryAllByRole("checkbox"), `${name} checkboxes`).toHaveLength(0);
      expect(within(container).queryAllByRole("switch"), `${name} switches`).toHaveLength(0);
      expect(
        container.querySelectorAll(
          "button, input, select, textarea, form, [role='button'], [role='checkbox'], [role='switch']"
        ),
        `${name} interactive controls`
      ).toHaveLength(0);

      unmount();
    }
  });
});

describe("panel placement", () => {
  it("renders the startup, DNS, and block-list panels on History", async () => {
    render(<HistoryScreen api={makeApi()} onOpenCategory={() => undefined} />);

    expect(
      await screen.findByRole("heading", { name: "Startup inventory" })
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "DNS resolver ranking" })
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Block-list plan" })
    ).toBeInTheDocument();
  });

  it("renders the privacy panel on Home", async () => {
    render(<HomeScreen api={makeApi()} />);

    await screen.findByRole("heading", { name: "Largest categories" });
    expect(
      await screen.findByRole("heading", { name: "Privacy findings" })
    ).toBeInTheDocument();
  });
});
