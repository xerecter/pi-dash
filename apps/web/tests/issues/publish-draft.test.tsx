/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { createRef } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { handleCreateUpdatePropertyValues, moveIssue, setToast } = vi.hoisted(() => ({
  handleCreateUpdatePropertyValues: vi.fn(),
  moveIssue: vi.fn(),
  setToast: vi.fn(),
}));

vi.mock("mobx-react", () => ({ observer: <T,>(component: T) => component }));
vi.mock("next/navigation", () => ({ useParams: () => ({ workspaceSlug: "acme" }) }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock("@karma-team/constants", () => ({
  DEFAULT_WORK_ITEM_FORM_VALUES: {
    name: "",
    project_id: "",
    type_id: null,
    description_html: "<p></p>",
    parent_id: null,
  },
  ETabIndices: { ISSUE_FORM: "issue-form" },
}));
vi.mock("@karma-team/services", () => ({
  AssistantService: class {
    generateTitle = vi.fn();
  },
}));
vi.mock("@karma-team/propel/toast", () => ({
  TOAST_TYPE: { ERROR: "ERROR", SUCCESS: "SUCCESS" },
  setToast,
}));
vi.mock("@karma-team/propel/button", async () => {
  const { forwardRef } = await import("react");
  return {
    Button: forwardRef<
      HTMLButtonElement,
      React.ButtonHTMLAttributes<HTMLButtonElement> & {
        loading?: boolean;
        variant?: string;
        size?: string;
      }
    >(function Button({ children, loading: _loading, variant: _variant, size: _size, ...props }, ref) {
      return (
        <button ref={ref} {...props}>
          {children}
        </button>
      );
    }),
  };
});
vi.mock("@karma-team/ui", () => ({
  ToggleSwitch: () => null,
}));
vi.mock("@karma-team/utils", () => ({
  cn: (...values: Array<string | false | null | undefined>) => values.filter(Boolean).join(" "),
  convertWorkItemDataToSearchResponse: vi.fn(),
  getChangedIssuefields: (data: unknown) => data,
  getTabIndex: () => ({ getIndex: () => 0 }),
  getTextContent: (value?: string) => value ?? "",
  getUpdateFormDataForReset: (_projectId: string, values: unknown) => values,
}));

vi.mock("@/components/assistant/use-llm-config", () => ({ useLLMConfig: () => ({ config: null }) }));
vi.mock("@/components/issues/issue-modal/components", () => ({
  IssueDefaultProperties: () => null,
  IssueDescriptionEditor: () => null,
  IssueGitAdvanced: () => null,
  IssueParentTag: () => null,
  IssueProjectSelect: () => null,
  IssueTitleInput: () => null,
}));
vi.mock("@/hooks/context/use-issue-modal", () => ({
  useIssueModal: () => ({
    workItemTemplateId: null,
    isApplyingTemplate: false,
    selectedParentIssue: null,
    setWorkItemTemplateId: vi.fn(),
    setSelectedParentIssue: vi.fn(),
    getIssueTypeIdOnProjectChange: vi.fn(),
    getActiveAdditionalPropertiesLength: () => 0,
    handlePropertyValuesValidation: () => true,
    handleCreateUpdatePropertyValues,
    handleTemplateChange: vi.fn(),
  }),
}));
vi.mock("@/hooks/store/use-issue-detail", () => ({
  useIssueDetail: () => ({ issue: { getIssueById: vi.fn() } }),
}));
vi.mock("@/hooks/store/use-project", () => ({
  useProject: () => ({ getProjectById: () => ({ workspace: "workspace-1" }) }),
}));
vi.mock("@/hooks/store/use-project-state", () => ({
  useProjectState: () => ({ getStateById: vi.fn() }),
}));
vi.mock("@/hooks/store/workspace-draft", () => ({
  useWorkspaceDraftIssues: () => ({ moveIssue }),
}));
vi.mock("@/hooks/use-platform-os", () => ({ usePlatformOS: () => ({ isMobile: false }) }));
vi.mock("@/hooks/use-project-issue-properties", () => ({
  useProjectIssueProperties: () => ({ fetchCycles: vi.fn() }),
}));
vi.mock("@/karma-team-web/hooks/use-debounced-duplicate-issues", () => ({
  useDebouncedDuplicateIssues: () => ({ duplicateIssues: [] }),
}));
vi.mock("@/karma-team-web/components/de-dupe/de-dupe-button", () => ({ DeDupeButtonRoot: () => null }));
vi.mock("@/karma-team-web/components/de-dupe/duplicate-modal", () => ({ DuplicateModalRoot: () => null }));
vi.mock("@/karma-team-web/components/issues/issue-modal", async () => {
  const { useFormContext } = await import("react-hook-form");
  return {
    IssueTypeSelect: () => {
      const { setValue } = useFormContext();
      return (
        <button type="button" onClick={() => setValue("type_id", "type-2")}>
          Select another type
        </button>
      );
    },
    WorkItemTemplateSelect: () => null,
  };
});
vi.mock("@/karma-team-web/components/issues/issue-modal/modal-additional-properties", () => ({
  WorkItemModalAdditionalProperties: () => null,
}));

import { IssueFormRoot } from "@/components/issues/issue-modal/form";

const renderDraft = (overrides: Record<string, unknown> = {}) => {
  const onClose = vi.fn();
  render(
    <IssueFormRoot
      data={{
        id: "draft-1",
        project_id: "project-1",
        type_id: "type-1",
        name: "Draft title",
        description_html: "<p>Draft body</p>",
        ...overrides,
      }}
      issueTitleRef={createRef<HTMLInputElement>()}
      isCreateMoreToggleEnabled={false}
      onAssetUpload={vi.fn()}
      onCreateMoreToggleChange={vi.fn()}
      onClose={onClose}
      onSubmit={vi.fn()}
      projectId="project-1"
      isDraft
      isDuplicateModalOpen={false}
      handleDuplicateIssueModal={vi.fn()}
    />
  );
  return { onClose };
};

describe("draft publishing", () => {
  beforeEach(() => {
    handleCreateUpdatePropertyValues.mockReset().mockResolvedValue(undefined);
    moveIssue.mockReset().mockResolvedValue({ id: "issue-1" });
    setToast.mockReset();
  });

  it("publishes an existing draft and closes the detail modal", async () => {
    const user = userEvent.setup();
    const { onClose } = renderDraft();

    await user.click(screen.getByRole("button", { name: "Publish issue" }));

    await waitFor(() => expect(moveIssue).toHaveBeenCalledTimes(1));
    expect(handleCreateUpdatePropertyValues).toHaveBeenCalledWith({
      issueId: "draft-1",
      issueTypeId: "type-1",
      projectId: "project-1",
      workspaceSlug: "acme",
      isDraft: true,
    });
    expect(moveIssue).toHaveBeenCalledWith(
      "acme",
      "draft-1",
      expect.objectContaining({ id: "draft-1", project_id: "project-1" })
    );
    expect(setToast).toHaveBeenCalledWith({
      type: "SUCCESS",
      title: "Success",
      message: "Draft published to project.",
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("publishes custom properties and the issue with the current issue type", async () => {
    const user = userEvent.setup();
    renderDraft();

    await user.click(screen.getByRole("button", { name: "Select another type" }));
    await user.click(screen.getByRole("button", { name: "Publish issue" }));

    await waitFor(() => expect(moveIssue).toHaveBeenCalledTimes(1));
    expect(handleCreateUpdatePropertyValues).toHaveBeenCalledWith(
      expect.objectContaining({
        issueTypeId: "type-2",
        projectId: "project-1",
      })
    );
    expect(moveIssue).toHaveBeenCalledWith("acme", "draft-1", expect.objectContaining({ type_id: "type-2" }));
  });

  it("keeps a draft open when publishing fails so the user can retry", async () => {
    const user = userEvent.setup();
    moveIssue.mockRejectedValueOnce(new Error("network"));
    const { onClose } = renderDraft();

    await user.click(screen.getByRole("button", { name: "Publish issue" }));

    await waitFor(() =>
      expect(setToast).toHaveBeenCalledWith({
        type: "ERROR",
        title: "Error!",
        message: "Failed to move work item to project. Please try again.",
      })
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Publish issue" })).toBeEnabled();
  });

  it("reports a missing project without issuing a move request", async () => {
    const user = userEvent.setup();
    renderDraft({ project_id: undefined });

    await user.click(screen.getByRole("button", { name: "Publish issue" }));

    expect(moveIssue).not.toHaveBeenCalled();
    expect(setToast).toHaveBeenCalledWith({
      type: "ERROR",
      title: "Error",
      message: "Select a project before publishing this draft.",
    });
  });
});
