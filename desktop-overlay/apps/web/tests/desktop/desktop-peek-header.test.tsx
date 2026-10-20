/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { observable, runInAction } from "mobx";
import { observer } from "mobx-react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

type TPeekIssue = { workspaceSlug: string; projectId: string; issueId: string };

const mocks = vi.hoisted(() => ({
  store: undefined as unknown as { peekIssue: TPeekIssue | undefined },
  issue: undefined as { project_id: string; sequence_id: number } | undefined,
  projectIdentifier: undefined as string | undefined,
  copy: vi.fn(),
}));

vi.mock("@karma-team/constants", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@karma-team/constants")>()),
  WEB_BASE_URL: "https://karma-team.example.com",
}));
vi.mock("@karma-team/utils", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@karma-team/utils")>()),
  copyTextToClipboard: mocks.copy,
}));
vi.mock("@karma-team/propel/toast", () => ({
  TOAST_TYPE: { SUCCESS: "success", ERROR: "error" },
  setToast: vi.fn(),
}));
vi.mock("@karma-team/propel/tooltip", () => ({
  Tooltip: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("@karma-team/propel/icon-button", () => ({
  IconButton: ({ onClick, disabled }: { onClick: React.MouseEventHandler<HTMLButtonElement>; disabled?: boolean }) => (
    <button type="button" aria-label="Copy link" onClick={onClick} disabled={disabled} />
  ),
}));
vi.mock("@karma-team/ui", () => ({ CustomSelect: Object.assign(() => null, { Option: () => null }) }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (message: string) => message }) }));
vi.mock("lucide-react", async (importOriginal) => ({
  ...(await importOriginal<typeof import("lucide-react")>()),
  MoveDiagonal: () => <svg data-testid="expand" />,
}));
vi.mock("@/hooks/store/user", () => ({ useUser: () => ({ data: { id: "user" } }) }));
vi.mock("@/hooks/store/use-issue-detail", () => ({
  useIssueDetail: () => ({
    get peekIssue() {
      return mocks.store.peekIssue;
    },
    setPeekIssue: (peekIssue: TPeekIssue | undefined) =>
      runInAction(() => {
        mocks.store.peekIssue = peekIssue;
      }),
    isAnyModalOpen: false,
    issue: { getIssueById: () => mocks.issue },
    removeIssue: vi.fn(),
    archiveIssue: vi.fn(),
    getIsIssuePeeked: () => true,
  }),
}));
vi.mock("@/hooks/store/use-issues", () => ({ useIssues: () => ({ issues: { removeIssue: vi.fn() } }) }));
vi.mock("@/hooks/store/use-project", () => ({
  useProject: () => ({ getProjectIdentifierById: () => mocks.projectIdentifier }),
}));
vi.mock("@/hooks/use-platform-os", () => ({ usePlatformOS: () => ({ isMobile: false }) }));
vi.mock("@/hooks/use-issue-layout-store", () => ({ useIssueStoreType: () => "PROJECT" }));
vi.mock("../../core/components/issues/issue-detail/subscription", () => ({ IssueSubscription: () => null }));
vi.mock("../../core/components/issues/issue-layouts/quick-action-dropdowns", () => ({
  WorkItemDetailQuickActions: () => null,
}));
vi.mock("../../core/components/issues/issue-update-status", () => ({ NameDescriptionUpdateStatus: () => null }));

import { IssuePeekOverviewHeader } from "../../core/components/issues/peek-overview/header";
import { IssuePeekUrlSync } from "../../core/components/issues/peek-overview/url-sync";

const LIST_PATH = "/acme/projects/project-id/issues/";

const Header = ({ isArchived = false }: { isArchived?: boolean }) => (
  <IssuePeekOverviewHeader
    peekMode="side-peek"
    setPeekMode={vi.fn()}
    removeRoutePeekId={() =>
      runInAction(() => {
        mocks.store.peekIssue = undefined;
      })
    }
    workspaceSlug="acme"
    projectId="project-id"
    issueId="issue-id"
    isArchived={isArchived}
    disabled={false}
    embedIssue={false}
    toggleDeleteIssueModal={vi.fn()}
    toggleArchiveIssueModal={vi.fn()}
    toggleDuplicateIssueModal={vi.fn()}
    toggleEditIssueModal={vi.fn()}
    toggleMoveIssueModal={vi.fn()}
    handleRestoreIssue={vi.fn()}
    isSubmitting="saved"
  />
);

const WorkItemList = observer(function WorkItemList() {
  return (
    <>
      <IssuePeekUrlSync />
      {mocks.store.peekIssue && <Header />}
    </>
  );
});

const lazyPage = async () => {
  await new Promise((resolve) => setTimeout(resolve, 5));
  return { Component: () => <div>work item detail</div> };
};

const renderApp = (element: React.ReactNode = <WorkItemList />) => {
  const router = createMemoryRouter(
    [
      { path: "/:workspaceSlug/projects/:projectId/issues/", element },
      { path: "/:workspaceSlug/browse/:workItem/", lazy: lazyPage },
    ],
    { initialEntries: [`${LIST_PATH}?peekIssueId=issue-id`] }
  );
  render(<RouterProvider router={router} />);
  return router;
};

const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 50));
  });

beforeEach(() => {
  mocks.store = observable({
    peekIssue: { workspaceSlug: "acme", projectId: "project-id", issueId: "issue-id" } as TPeekIssue | undefined,
  });
  mocks.issue = { project_id: "project-id", sequence_id: 7 };
  mocks.projectIdentifier = "DESK";
  mocks.copy.mockReset().mockResolvedValue(undefined);
});

describe("desktop peek header", () => {
  it("opens the loaded work item and leaves the list history entry clean", async () => {
    const router = renderApp();
    const expand = (await screen.findByTestId("expand")).closest("a");
    expect(expand?.getAttribute("href")).toBe("/acme/browse/DESK-7/");

    fireEvent.click(expand!);
    await settle();
    expect(router.state.location.pathname).toBe("/acme/browse/DESK-7/");

    await act(async () => router.navigate(-1));
    await waitFor(() => expect(router.state.location.pathname).toBe(LIST_PATH));
    expect(router.state.location.search).toBe("");
  });

  it("does not expose open or copy actions while a regular item link is unresolved", async () => {
    mocks.projectIdentifier = undefined;
    const router = renderApp(<Header />);

    const icon = await screen.findByTestId("expand");
    expect(icon.closest("a")).toBeNull();
    expect(icon.closest("[aria-disabled]")).not.toBeNull();
    const copy = screen.getByRole("button", { name: "Copy link" }) as HTMLButtonElement;
    expect(copy.disabled).toBe(true);

    fireEvent.click(icon);
    fireEvent.click(copy);
    await settle();
    expect(router.state.location.pathname).toBe(LIST_PATH);
    expect(mocks.copy).not.toHaveBeenCalled();
  });

  it("uses the route project id for an archived item before details load", async () => {
    mocks.issue = undefined;
    mocks.projectIdentifier = undefined;
    renderApp(<Header isArchived />);

    const expand = (await screen.findByTestId("expand")).closest("a");
    expect(expand?.getAttribute("href")).toBe("/acme/projects/project-id/archives/issues/issue-id/");
  });

  it("copies the hosted URL instead of the desktop bundle origin", async () => {
    renderApp(<Header />);
    fireEvent.click(await screen.findByRole("button", { name: "Copy link" }));
    await settle();

    expect(mocks.copy).toHaveBeenCalledWith("https://karma-team.example.com/acme/browse/DESK-7/");
  });
});
