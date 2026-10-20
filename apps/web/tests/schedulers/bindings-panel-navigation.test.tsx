import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ISchedulerBinding } from "@karma-team/services";

const { useSWR, navigate, updateBinding } = vi.hoisted(() => ({
  useSWR: vi.fn(),
  navigate: vi.fn(),
  updateBinding: vi.fn(),
}));

vi.mock("swr", () => ({ default: useSWR }));
vi.mock("react-router", () => ({ useNavigate: () => navigate }));
vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
vi.mock("@karma-team/constants", () => ({
  EUserPermissions: { ADMIN: 20 },
  EUserPermissionsLevel: { PROJECT: "PROJECT", WORKSPACE: "WORKSPACE" },
}));
vi.mock("@karma-team/propel/toast", () => ({
  TOAST_TYPE: { ERROR: "error", SUCCESS: "success" },
  setToast: vi.fn(),
}));
vi.mock("@karma-team/services", () => ({
  SchedulerService: class {
    listBindings = vi.fn();
    listSchedulers = vi.fn();
    updateBinding = updateBinding;
  },
}));
vi.mock("@karma-team/ui", () => ({
  Button: ({ children, onClick }: { children: React.ReactNode; onClick?: () => void }) => (
    <button type="button" onClick={onClick}>
      {children}
    </button>
  ),
  ToggleSwitch: ({ value, onChange }: { value: boolean; onChange: (next: boolean) => void }) => (
    <input role="switch" type="checkbox" checked={value} onChange={() => onChange(!value)} />
  ),
}));
vi.mock("@/components/project/scheduler-bindings/edit-binding-modal", () => ({
  EditSchedulerBindingModal: () => null,
}));
vi.mock("@/components/project/scheduler-bindings/new-scheduler-modal", () => ({ NewSchedulerModal: () => null }));
vi.mock("@/components/project/scheduler-bindings/uninstall-binding-modal", () => ({
  UninstallSchedulerBindingModal: () => null,
}));
vi.mock("@/hooks/store/user", () => ({ useUserPermissions: () => ({ allowPermissions: () => true }) }));

import { SchedulerBindingsPanel } from "@/components/project/scheduler-bindings/bindings-panel";

const binding = {
  id: "binding-1",
  scheduler: "scheduler-1",
  scheduler_slug: "nightly-audit",
  scheduler_name: "Nightly Audit",
  scheduler_color: "#10b981",
  project: "project-1",
  workspace: "workspace-1",
  dtstart: "2026-09-01T09:00:00Z",
  tzid: "UTC",
  rrule: "FREQ=DAILY",
  rdates: [],
  exdates: [],
  extra_context: "",
  enabled: true,
  outcome_mode: "create_issue",
  pod: null,
  pod_name: null,
  next_run_at: null,
  last_run: null,
  last_run_status: null,
  last_run_ended_at: null,
  last_error: "",
  actor: null,
  created_at: "2026-09-01T00:00:00Z",
  updated_at: "2026-09-01T00:00:00Z",
} as ISchedulerBinding;

describe("SchedulerBindingsPanel navigation", () => {
  beforeEach(() => {
    navigate.mockReset();
    updateBinding.mockReset().mockResolvedValue(binding);
    useSWR.mockImplementation((key: unknown) => {
      const kind = Array.isArray(key) ? key[0] : null;
      if (kind === "scheduler-bindings") {
        return {
          data: [binding],
          mutate: vi.fn((updater?: unknown) => {
            if (typeof updater === "function") {
              return (updater as (current: ISchedulerBinding[]) => unknown)([binding]);
            }
            return Promise.resolve();
          }),
        };
      }
      if (kind === "schedulers") return { data: [], mutate: vi.fn() };
      return { data: undefined, mutate: vi.fn() };
    });
  });

  it("opens the binding detail route from a row click", () => {
    render(<SchedulerBindingsPanel workspaceSlug="acme" projectId="project-1" />);

    fireEvent.click(screen.getByText("Nightly Audit"));
    expect(navigate).toHaveBeenCalledWith("/acme/projects/project-1/schedulers/binding-1");
  });

  it("does not navigate when using row management controls", async () => {
    render(<SchedulerBindingsPanel workspaceSlug="acme" projectId="project-1" />);

    fireEvent.click(screen.getByText("Edit"));
    fireEvent.click(screen.getByText("Uninstall"));
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() => expect(updateBinding).toHaveBeenCalled());
    expect(navigate).not.toHaveBeenCalled();
  });
});
