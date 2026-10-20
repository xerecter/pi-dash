import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IScheduler, ISchedulerBinding } from "@karma-team/services";

const { createScheduler, createBinding, setToast } = vi.hoisted(() => ({
  createScheduler: vi.fn(),
  createBinding: vi.fn(),
  setToast: vi.fn(),
}));

vi.mock("@karma-team/services", () => ({
  SchedulerService: class {
    createScheduler = createScheduler;
    createBinding = createBinding;
  },
}));

vi.mock("@karma-team/i18n", () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars ? key.replace(/\{(\w+)\}/g, (_match, name) => String(vars[name] ?? "")) : key,
  }),
}));

vi.mock("@karma-team/propel/toast", () => ({
  TOAST_TYPE: { ERROR: "ERROR", SUCCESS: "SUCCESS" },
  setToast,
}));

vi.mock("@karma-team/propel/button", () => ({
  Button: ({
    children,
    loading: _loading,
    variant: _variant,
    ...props
  }: {
    children: React.ReactNode;
    loading?: boolean;
    variant?: string;
  } & React.ButtonHTMLAttributes<HTMLButtonElement>) => <button {...props}>{children}</button>,
}));

vi.mock("@karma-team/ui", async () => {
  const { forwardRef } = await import("react");
  return {
    ModalCore: ({ isOpen, children }: { isOpen: boolean; children: React.ReactNode }) =>
      isOpen ? <div role="dialog">{children}</div> : null,
    EModalPosition: { CENTER: "CENTER" },
    EModalWidth: { XL: "XL", XXL: "XXL" },
    Input: forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement> & { hasError?: boolean }>(
      function Input({ hasError: _hasError, ...props }, ref) {
        return <input ref={ref} {...props} />;
      }
    ),
    TextArea: forwardRef<
      HTMLTextAreaElement,
      React.TextareaHTMLAttributes<HTMLTextAreaElement> & { hasError?: boolean }
    >(function TextArea({ hasError: _hasError, ...props }, ref) {
      return <textarea ref={ref} {...props} />;
    }),
  };
});

vi.mock("@/components/project/scheduler-bindings/binding-schedule-fields", () => ({
  BindingScheduleFields: () => null,
}));
vi.mock("@/components/project/scheduler-bindings/binding-outcome-mode-field", () => ({
  BindingOutcomeModeField: () => null,
  DEFAULT_OUTCOME_MODE: "create_issue",
}));
vi.mock("@/components/project/scheduler-bindings/binding-pod-field", () => ({
  BindingPodField: () => null,
}));

import { NewSchedulerModal } from "@/components/project/scheduler-bindings/new-scheduler-modal";

const existingScheduler = {
  id: "sched-existing",
  slug: "existing",
  name: "Existing",
  is_enabled: true,
} as IScheduler;

function renderModal(overrides: Partial<React.ComponentProps<typeof NewSchedulerModal>> = {}) {
  const onClose = vi.fn();
  const onInstalled = vi.fn();
  const onSchedulersChanged = vi.fn();
  render(
    <NewSchedulerModal
      isOpen
      onClose={onClose}
      workspaceSlug="acme"
      projectId="project-1"
      availableSchedulers={[existingScheduler]}
      existingBindings={[] as ISchedulerBinding[]}
      canCreateScheduler
      onInstalled={onInstalled}
      onSchedulersChanged={onSchedulersChanged}
      {...overrides}
    />
  );
  return { onClose, onInstalled, onSchedulersChanged };
}

describe("NewSchedulerModal", () => {
  beforeEach(() => {
    createScheduler.mockReset();
    createBinding.mockReset();
    setToast.mockReset();
  });

  it("installs an existing definition with Karma Team binding options intact", async () => {
    const user = userEvent.setup();
    createBinding.mockResolvedValue({ id: "binding-1" });
    const { onInstalled, onClose } = renderModal();

    await user.click(screen.getByRole("button", { name: "Install" }));

    await waitFor(() =>
      expect(createBinding).toHaveBeenCalledWith(
        "acme",
        "project-1",
        expect.objectContaining({
          scheduler: "sched-existing",
          project: "project-1",
          rrule: "FREQ=DAILY",
          outcome_mode: "create_issue",
          pod: null,
        })
      )
    );
    expect(onInstalled).toHaveBeenCalledWith({ id: "binding-1" });
    expect(onClose).toHaveBeenCalled();
  });

  it("selects the first installable scheduler when the catalog arrives after opening", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const onInstalled = vi.fn();
    const onSchedulersChanged = vi.fn();
    createBinding.mockResolvedValue({ id: "binding-1" });
    const modal = (availableSchedulers: IScheduler[]) => (
      <NewSchedulerModal
        isOpen
        onClose={onClose}
        workspaceSlug="acme"
        projectId="project-1"
        availableSchedulers={availableSchedulers}
        existingBindings={[] as ISchedulerBinding[]}
        canCreateScheduler={false}
        onInstalled={onInstalled}
        onSchedulersChanged={onSchedulersChanged}
      />
    );
    const { rerender } = render(modal([]));

    rerender(modal([existingScheduler]));
    await waitFor(() => expect(screen.getByLabelText("Scheduler")).toHaveValue("sched-existing"));
    await user.click(screen.getByRole("button", { name: "Install" }));

    await waitFor(() =>
      expect(createBinding).toHaveBeenCalledWith(
        "acme",
        "project-1",
        expect.objectContaining({ scheduler: "sched-existing" })
      )
    );
  });

  it("creates a definition and installs it from the project modal", async () => {
    const user = userEvent.setup();
    const created = { ...existingScheduler, id: "sched-new", slug: "weekly-review", name: "Weekly Review" };
    createScheduler.mockResolvedValue(created);
    createBinding.mockResolvedValue({ id: "binding-new" });
    const { onSchedulersChanged, onInstalled } = renderModal({ availableSchedulers: [] });

    await user.type(screen.getByLabelText("Name"), "Weekly Review");
    await user.type(screen.getByLabelText("Prompt"), "Review this project");
    expect(screen.getByLabelText("Slug")).toHaveValue("weekly-review");
    await user.click(screen.getByRole("button", { name: "Create & install" }));

    await waitFor(() => expect(createScheduler).toHaveBeenCalled());
    expect(createBinding).toHaveBeenCalledWith(
      "acme",
      "project-1",
      expect.objectContaining({ scheduler: "sched-new" })
    );
    expect(onSchedulersChanged).toHaveBeenCalled();
    expect(onInstalled).toHaveBeenCalledWith({ id: "binding-new" });
  });

  it("keeps a newly created definition selected when installation fails, then retries without recreating", async () => {
    const user = userEvent.setup();
    const created = { ...existingScheduler, id: "sched-new", slug: "weekly-review", name: "Weekly Review" };
    createScheduler.mockResolvedValue(created);
    createBinding.mockRejectedValueOnce({ rrule: ["Invalid schedule"] }).mockResolvedValueOnce({ id: "binding-new" });
    const { onInstalled } = renderModal({ availableSchedulers: [] });

    await user.type(screen.getByLabelText("Name"), "Weekly Review");
    await user.type(screen.getByLabelText("Prompt"), "Review this project");
    await user.click(screen.getByRole("button", { name: "Create & install" }));

    expect(await screen.findByText("Scheduler definition saved")).toBeInTheDocument();
    expect(screen.getByText("Invalid schedule")).toBeInTheDocument();
    expect(screen.getByLabelText("Scheduler")).toHaveValue("sched-new");
    await user.click(screen.getByRole("button", { name: "Retry install" }));

    await waitFor(() => expect(onInstalled).toHaveBeenCalledWith({ id: "binding-new" }));
    expect(createScheduler).toHaveBeenCalledTimes(1);
    expect(createBinding).toHaveBeenCalledTimes(2);
  });

  it("does not offer definition creation to non-workspace-admins", () => {
    renderModal({ canCreateScheduler: false });

    expect(screen.queryByRole("tab", { name: "Create new" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Scheduler")).toBeInTheDocument();
  });
});
