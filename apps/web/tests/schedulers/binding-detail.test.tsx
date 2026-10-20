import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ISchedulerBinding } from "@karma-team/services";
import type { IAgentRun, IAgentRunPage } from "@karma-team/types";

const { useSWR, navigate } = vi.hoisted(() => ({
  useSWR: vi.fn(),
  navigate: vi.fn(),
}));

vi.mock("swr", () => ({ default: useSWR }));
vi.mock("react-router", () => ({
  useNavigate: () => navigate,
  Link: ({ to, children }: { to: string; children: React.ReactNode }) => <a href={to}>{children}</a>,
}));
vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("@karma-team/i18n", () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars ? key.replace(/\{(\w+)\}/g, (_match, name) => String(vars[name] ?? "")) : key,
  }),
}));
vi.mock("@karma-team/constants", () => ({
  EUserPermissions: { ADMIN: 20 },
  EUserPermissionsLevel: { PROJECT: "PROJECT", WORKSPACE: "WORKSPACE" },
}));
vi.mock("@karma-team/propel/toast", () => ({ TOAST_TYPE: { ERROR: "error" }, setToast: vi.fn() }));
vi.mock("@karma-team/services", () => ({
  SchedulerService: class {
    retrieveBinding = vi.fn();
    listBindingRuns = vi.fn();
    updateBinding = vi.fn();
  },
}));
vi.mock("@karma-team/ui", () => ({
  Badge: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
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
  ToggleSwitch: ({
    value,
    onChange,
    disabled,
  }: {
    value: boolean;
    onChange: (next: boolean) => void;
    disabled?: boolean;
  }) => <input role="switch" type="checkbox" checked={value} disabled={disabled} onChange={() => onChange(!value)} />,
}));
vi.mock("@/components/project/scheduler-bindings/edit-binding-modal", () => ({
  EditSchedulerBindingModal: () => null,
}));
vi.mock("@/components/project/scheduler-bindings/uninstall-binding-modal", () => ({
  UninstallSchedulerBindingModal: () => null,
}));
vi.mock("@/hooks/store/user", () => ({ useUserPermissions: () => ({ allowPermissions: () => true }) }));
vi.mock("@/hooks/store/use-member", () => ({
  useMember: () => ({ getUserDetails: () => ({ display_name: "Ada" }) }),
}));

import { SchedulerBindingDetail } from "@/components/project/scheduler-bindings/binding-detail";

function makeBinding(overrides: Partial<ISchedulerBinding> = {}): ISchedulerBinding {
  return {
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
    extra_context: "Focus on the API.",
    enabled: true,
    outcome_mode: "create_issue",
    pod: null,
    pod_name: null,
    next_run_at: "2026-09-27T09:00:00Z",
    last_run: null,
    last_run_status: null,
    last_run_ended_at: null,
    last_error: "",
    actor: "user-1",
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-01T00:00:00Z",
    resolved_prompt: "Audit the project.\n\nFocus on the API.",
    run_count: 1,
    scheduler_source: "builtin",
    scheduler_is_enabled: true,
    ...overrides,
  };
}

function makeRun(): IAgentRun {
  return {
    id: "run-1",
    status: "completed",
    started_at: "2026-09-26T09:00:01Z",
    ended_at: "2026-09-26T09:03:01Z",
    created_at: "2026-09-26T09:00:00Z",
    error: "",
    done_payload: { summary: "Filed 2 issues" },
    pod_detail: { name: "WEB_pod_1" },
    scheduler_binding: "binding-1",
  } as unknown as IAgentRun;
}

function makePage(results: IAgentRun[], overrides: Partial<IAgentRunPage> = {}): IAgentRunPage {
  return {
    results,
    count: results.length,
    total_count: results.length,
    total_pages: 1,
    page: 1,
    per_page: 30,
    ...overrides,
  };
}

function mockData(
  binding: ISchedulerBinding | undefined,
  runs: IAgentRunPage | undefined,
  bindingError?: unknown,
  runsError?: unknown,
  mutateRuns = vi.fn()
) {
  useSWR.mockImplementation((key: unknown) => {
    const kind = Array.isArray(key) ? key[0] : null;
    if (kind === "scheduler-binding-detail") return { data: binding, error: bindingError, mutate: vi.fn() };
    if (kind === "scheduler-binding-runs") return { data: runs, error: runsError, mutate: mutateRuns };
    return { data: undefined, mutate: vi.fn() };
  });
}

function renderDetail() {
  return render(<SchedulerBindingDetail workspaceSlug="acme" projectId="project-1" bindingId="binding-1" />);
}

describe("SchedulerBindingDetail", () => {
  beforeEach(() => {
    useSWR.mockReset();
    navigate.mockReset();
  });

  it("renders binding configuration and opens a history run", () => {
    mockData(makeBinding(), makePage([makeRun()]));
    renderDetail();

    expect(screen.getByText("Nightly Audit")).toBeInTheDocument();
    expect(screen.getByText("(default pod)")).toBeInTheDocument();
    expect(screen.getByText("Ada")).toBeInTheDocument();
    expect(screen.getByText("WEB_pod_1")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Filed 2 issues"));
    expect(navigate).toHaveBeenCalledWith("/acme/projects/project-1/runners/runs/run-1");
  });

  it("reveals the server-resolved prompt and handles empty history", () => {
    mockData(makeBinding(), makePage([]));
    renderDetail();

    expect(screen.queryByText(/Audit the project/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Show resolved prompt"));
    expect(screen.getByText(/Audit the project/)).toBeInTheDocument();
    expect(screen.getByText(/No runs yet - next run at/)).toBeInTheDocument();
  });

  it("requests the next run-history page", () => {
    mockData(makeBinding(), makePage([makeRun()], { total_count: 31, total_pages: 2 }));
    renderDetail();

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(useSWR).toHaveBeenCalledWith(
      ["scheduler-binding-runs", "acme", "project-1", "binding-1", 2],
      expect.any(Function),
      expect.any(Object)
    );
  });

  it("shows a safe unavailable state for deleted or inaccessible bindings", () => {
    mockData(undefined, undefined, { error: "not found" });
    renderDetail();

    expect(
      screen.getByText("This scheduler install is not available. It may have been uninstalled.")
    ).toBeInTheDocument();
  });

  it("shows a retry action when run history fails to load", () => {
    const mutateRuns = vi.fn();
    mockData(makeBinding(), undefined, undefined, new Error("history unavailable"), mutateRuns);
    renderDetail();

    expect(screen.getByText("Could not load run history.")).toBeInTheDocument();
    expect(screen.queryByText("Loading…")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(mutateRuns).toHaveBeenCalledTimes(1);
  });
});
