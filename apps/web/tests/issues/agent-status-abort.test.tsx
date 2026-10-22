/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef, useState } from "react";
import { observable, runInAction } from "mobx";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TIssue } from "@karma-team/types";

const { abortRun, reTick } = vi.hoisted(() => ({
  abortRun: vi.fn(),
  reTick: vi.fn(),
}));

vi.mock("@karma-team/constants", () => ({
  API_BASE_URL: "",
}));

vi.mock("@karma-team/utils", () => ({
  cn: (...args: unknown[]) => args.filter(Boolean).join(" "),
}));

vi.mock("@karma-team/i18n", () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars ? key.replace(/\{(\w+)\}/g, (_m, k) => String(vars[k] ?? "")) : key,
  }),
}));

vi.mock("@karma-team/propel/toast", () => ({
  TOAST_TYPE: { SUCCESS: "success", ERROR: "error", INFO: "info" },
  setToast: vi.fn(),
}));

vi.mock("@karma-team/propel/badge", () => ({
  Badge: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
}));

vi.mock("@karma-team/propel/button", () => ({
  Button: ({
    children,
    onClick,
    disabled,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => (
    <button type="button" onClick={onClick} disabled={disabled}>
      {children}
    </button>
  ),
}));

// Both use-abort-run and use-re-tick construct an AgentRunService at module
// load; mock the shared service so the hooks call our spies.
vi.mock("@/services/runner", () => ({
  AgentRunService: class {
    abortRun = abortRun;
    reTick = reTick;
  },
}));

import { IssueAgentStatusPanel } from "../../core/components/issues/issue-detail/agent-status";
import usePeekOverviewOutsideClickDetector from "../../core/hooks/use-peek-overview-outside-click";
import { setToast } from "@karma-team/propel/toast";

const ISSUE_OPS = { fetch: vi.fn() };

function makeIssue(status: string): TIssue {
  return {
    id: "issue-1",
    agent_status: {
      ticker: null,
      active_run: {
        id: "run-1",
        status,
        runner: "runner-1",
        runner_name: "workx_claude01",
        created_at: "2026-07-21T00:00:00Z",
        assigned_at: null,
        started_at: null,
        ended_at: null,
        done_payload: null,
        error: "",
        error_diagnostic: null,
        llm_model: "",
        input_tokens: null,
        output_tokens: null,
        total_tokens: null,
      },
      latest_run: null,
      run_count: 1,
    },
  } as unknown as TIssue;
}

function PeekPanel({ issue }: { issue: TIssue }) {
  const ref = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(true);
  usePeekOverviewOutsideClickDetector(ref, () => setOpen(false), issue.id);
  return open ? (
    <div ref={ref} data-testid="peek">
      <IssueAgentStatusPanel
        workspaceSlug="acme"
        projectId="proj-1"
        issueId="issue-1"
        issue={issue}
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        issueOperations={ISSUE_OPS as any}
      />
    </div>
  ) : (
    <span>Peek closed</span>
  );
}

function renderPanel(issue: TIssue) {
  return render(<PeekPanel issue={issue} />);
}

describe("IssueAgentStatusPanel abort run", () => {
  beforeEach(() => {
    abortRun.mockReset();
    reTick.mockReset();
    ISSUE_OPS.fetch.mockReset().mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("shows the abort button while a run is active", () => {
    renderPanel(makeIssue("running"));
    expect(screen.getByText("Abort run")).toBeTruthy();
  });

  it("confirming abort signals the runner and refreshes the card", async () => {
    abortRun.mockResolvedValue({ id: "run-1", status: "cancelled" });
    renderPanel(makeIssue("running"));

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Abort run" }));
    await user.click(
      within(screen.getByRole("group", { name: "Abort run?" })).getByRole("button", { name: "Abort run" })
    );

    await waitFor(() => expect(abortRun).toHaveBeenCalledWith("run-1", "user"));
    await waitFor(() => expect(ISSUE_OPS.fetch).toHaveBeenCalledWith("acme", "proj-1", "issue-1"));
    expect(screen.getByTestId("peek")).toBeTruthy();
    expect(screen.getByText("AI agent run was cancelled")).toBeTruthy();
    expect(setToast).toHaveBeenCalledWith(expect.objectContaining({ title: "Run cancelled" }));
  });

  it("hides the abort button once the run is terminal", () => {
    renderPanel(makeIssue("cancelled"));
    expect(screen.queryByText("Abort run")).toBeNull();
  });

  it("keeps running when confirmation is dismissed", async () => {
    const user = userEvent.setup();
    renderPanel(makeIssue("running"));
    await user.click(screen.getByRole("button", { name: "Abort run" }));
    await user.click(screen.getByRole("button", { name: "Keep running" }));
    expect(abortRun).not.toHaveBeenCalled();
    expect(screen.queryByRole("group", { name: "Abort run?" })).toBeNull();
    expect(screen.getByTestId("peek")).toBeTruthy();
  });

  it("shows submission feedback and blocks repeated requests", async () => {
    let resolve!: (value: unknown) => void;
    abortRun.mockImplementation(
      () =>
        new Promise((r) => {
          resolve = r;
        })
    );
    renderPanel(makeIssue("running"));
    fireEvent.click(screen.getByRole("button", { name: "Abort run" }));
    fireEvent.click(screen.getByRole("button", { name: "Abort run" }));
    expect(screen.getByRole("button", { name: "Aborting" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Keep running" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Aborting" }));
    expect(abortRun).toHaveBeenCalledTimes(1);
    await act(async () => resolve({ id: "run-1", status: "cancel_requested" }));
    expect(screen.getByText("Stopping the AI agent")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Abort run" })).toBeNull();
    expect(setToast).toHaveBeenCalledWith(expect.objectContaining({ title: "Abort requested" }));
  });

  it("shows a persistent error and allows a retry", async () => {
    abortRun
      .mockRejectedValueOnce({ error: "Runner request failed" })
      .mockResolvedValueOnce({ id: "run-1", status: "cancel_requested" });
    renderPanel(makeIssue("running"));
    fireEvent.click(screen.getByRole("button", { name: "Abort run" }));
    fireEvent.click(screen.getByRole("button", { name: "Abort run" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Runner request failed"));
    expect(screen.getByRole("button", { name: "Abort run" })).not.toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Abort run" }));
    await waitFor(() => expect(screen.getByText("Stopping the AI agent")).toBeTruthy());
    expect(screen.queryByRole("alert")).toBeNull();
    expect(abortRun).toHaveBeenCalledTimes(2);
  });

  it("updates the badge when MobX changes the same issue object", async () => {
    const issue = observable(makeIssue("running"));
    renderPanel(issue);
    act(() =>
      runInAction(() => {
        issue.agent_status!.active_run!.status = "cancel_requested";
      })
    );
    expect(screen.getByText("Stopping the AI agent")).toBeTruthy();
    act(() =>
      runInAction(() => {
        issue.agent_status!.latest_run = { ...issue.agent_status!.active_run!, status: "cancelled" };
        issue.agent_status!.active_run = null;
      })
    );
    expect(screen.getByText("AI agent run was cancelled")).toBeTruthy();
  });

  it("polls a pending cancellation every two seconds even if refresh fails", async () => {
    vi.useFakeTimers();
    ISSUE_OPS.fetch.mockRejectedValue(new Error("temporary refresh failure"));
    renderPanel(makeIssue("cancel_requested"));
    await act(async () => vi.advanceTimersByTime(2_000));
    expect(ISSUE_OPS.fetch).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTime(2_000));
    expect(ISSUE_OPS.fetch).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Stopping the AI agent")).toBeTruthy();
  });
});
