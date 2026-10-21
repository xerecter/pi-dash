/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { InputHTMLAttributes, PropsWithChildren } from "react";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TIssue } from "@karma-team/types";

const { fetchIssue } = vi.hoisted(() => ({ fetchIssue: vi.fn() }));
vi.mock("mobx-react", () => ({ observer: <T,>(component: T) => component }));
vi.mock("next/navigation", () => ({ useParams: () => ({ workspaceSlug: "acme", projectId: "project-1" }) }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock("@karma-team/ui", () => ({
  EModalPosition: { TOP: "top" },
  EModalWidth: { VIXL: "wide", XXXXL: "normal" },
  ModalCore: ({ children, isOpen }: PropsWithChildren<{ isOpen: boolean }>) => (isOpen ? children : null),
  Input: ({ hasError: _hasError, ...props }: InputHTMLAttributes<HTMLInputElement> & { hasError?: boolean }) => (
    <input {...props} />
  ),
}));
vi.mock("@karma-team/propel/toast", () => ({ TOAST_TYPE: {}, setToast: vi.fn() }));
vi.mock("@/hooks/context/use-issue-modal", () => ({
  useIssueModal: () => ({ allowedProjectIds: ["project-1"] }),
}));
vi.mock("@/hooks/store/use-issue-detail", () => ({ useIssueDetail: () => ({ fetchIssue }) }));
vi.mock("@/hooks/store/use-cycle", () => ({ useCycle: () => ({}) }));
vi.mock("@/hooks/store/use-module", () => ({ useModule: () => ({}) }));
vi.mock("@/hooks/store/use-issues", () => ({ useIssues: () => ({ issues: {} }) }));
vi.mock("@/hooks/store/use-project", () => ({ useProject: () => ({ getProjectByIdentifier: vi.fn() }) }));
vi.mock("@/hooks/use-issue-layout-store", () => ({ useIssueStoreType: () => "PROJECT" }));
vi.mock("@/hooks/use-issues-actions", () => ({ useIssuesActions: () => ({}) }));
vi.mock("@/services/file.service", () => ({ FileService: vi.fn(function FileService() {}) }));
vi.mock("@/components/issues/create-issue-toast-action-items", () => ({ CreateIssueToastActionItems: () => null }));
vi.mock("@/components/issues/issue-modal/draft-issue-layout", () => ({ DraftIssueLayout: () => null }));
vi.mock("@/components/issues/issue-modal/form", () => ({
  // Uncontrolled input deliberately observes only the initial form data.
  IssueFormRoot: ({ data }: { data: Partial<TIssue> }) => (
    <input aria-label="Initial branch" defaultValue={data.git_work_branch ?? ""} />
  ),
}));

import { CreateUpdateIssueModalBase } from "@/components/issues/issue-modal/base";
import { IssueGitAdvanced } from "@/components/issues/issue-modal/components/issue-git-advanced";
import { getChangedIssuefields } from "@karma-team/utils";

const modalProps = {
  data: { id: "issue-1", project_id: "project-1" },
  isOpen: true,
  onClose: vi.fn(),
  withDraftIssueWrapper: false,
};

describe("work branch in the edit modal", () => {
  beforeEach(() => fetchIssue.mockReset());

  it("initializes from fetched details when list data omits the branch", async () => {
    let resolveDetails!: (data: Partial<TIssue>) => void;
    const details = new Promise<Partial<TIssue>>((resolve) => {
      resolveDetails = resolve;
    });
    fetchIssue.mockReturnValue(details);
    render(<CreateUpdateIssueModalBase {...modalProps} />);
    expect(screen.queryByLabelText("Initial branch")).toBeNull();
    await act(async () => resolveDetails({ description_html: "<p>Body</p>", git_work_branch: "feat/pinned" }));
    expect(await screen.findByLabelText("Initial branch")).toHaveValue("feat/pinned");
  });

  it("prefers an explicit empty value from the server over stale modal props", async () => {
    fetchIssue.mockResolvedValue({ description_html: "<p>Body</p>", git_work_branch: "" });
    render(<CreateUpdateIssueModalBase {...modalProps} data={{ ...modalProps.data, git_work_branch: "old" }} />);
    expect(await screen.findByLabelText("Initial branch")).toHaveValue("");
  });

  it("uses supplied details when fetching is disabled", async () => {
    render(
      <CreateUpdateIssueModalBase
        {...modalProps}
        fetchIssueDetails={false}
        data={{ ...modalProps.data, git_work_branch: "feat/supplied" }}
      />
    );
    expect(await screen.findByLabelText("Initial branch")).toHaveValue("feat/supplied");
    expect(fetchIssue).not.toHaveBeenCalled();
  });

  it("ignores an old detail response after switching work items", async () => {
    let resolveOld!: (data: Partial<TIssue>) => void;
    const oldDetails = new Promise<Partial<TIssue>>((resolve) => {
      resolveOld = resolve;
    });
    fetchIssue.mockImplementation((_workspace: string, _project: string, id: string) =>
      id === "issue-1" ? oldDetails : Promise.resolve({ git_work_branch: "feat/new" })
    );
    const { rerender } = render(<CreateUpdateIssueModalBase {...modalProps} />);
    rerender(<CreateUpdateIssueModalBase {...modalProps} data={{ ...modalProps.data, id: "issue-2" }} />);
    expect(await screen.findByLabelText("Initial branch")).toHaveValue("feat/new");
    await act(async () => resolveOld({ git_work_branch: "feat/old" }));
    expect(screen.getByLabelText("Initial branch")).toHaveValue("feat/new");
  });

  it("lets the user retry a failed detail request", async () => {
    fetchIssue.mockRejectedValue(new Error("Network error"));
    render(<CreateUpdateIssueModalBase {...modalProps} />);
    await screen.findByRole("alert");
    expect(screen.queryByLabelText("Initial branch")).toBeNull();
    fetchIssue.mockResolvedValue({ git_work_branch: "feat/retried" });
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByLabelText("Initial branch")).toHaveValue("feat/retried");
  });
});

function BranchForm({ onSubmit }: { onSubmit: (data: Partial<TIssue>) => void }) {
  const {
    control,
    register,
    handleSubmit,
    formState: { dirtyFields },
  } = useForm<TIssue>({
    defaultValues: { name: "Title", git_work_branch: "feat/pinned" },
  });
  return (
    <form
      onSubmit={handleSubmit((data) => onSubmit(getChangedIssuefields(data, dirtyFields as Record<string, boolean>)))}
    >
      <input aria-label="Title" {...register("name")} />
      <IssueGitAdvanced control={control} handleFormChange={() => undefined} />
      <button type="submit">Save</button>
    </form>
  );
}

describe("work branch form changes", () => {
  it("shows the saved branch and omits it from an unrelated title edit", async () => {
    const submit = vi.fn();
    render(<BranchForm onSubmit={submit} />);
    expect(screen.getByDisplayValue("feat/pinned")).toBeVisible();
    await userEvent.type(screen.getByLabelText("Title"), " changed");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledWith({ name: "Title changed" }));
  });

  it("retains the value after collapsing and explicitly submits a user clear", async () => {
    const submit = vi.fn();
    render(<BranchForm onSubmit={submit} />);
    await userEvent.click(screen.getByRole("button", { name: "Advanced — git" }));
    await userEvent.click(screen.getByRole("button", { name: "Advanced — git" }));
    await userEvent.clear(screen.getByDisplayValue("feat/pinned"));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledWith({ git_work_branch: "" }));
  });
});
