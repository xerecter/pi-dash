/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { ComponentProps, ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CliInstallConfig } from "@/services/cli-install.service";

const { configState } = vi.hoisted(() => ({
  configState: {
    data: undefined as CliInstallConfig | undefined,
    error: undefined as Error | undefined,
    mutate: vi.fn(),
  },
}));

vi.mock("@/hooks/use-cli-install-config", () => ({ useCliInstallConfig: () => configState }));
vi.mock("@/hooks/use-platform-os", () => ({ usePlatformOS: () => ({ platform: "Linux" }) }));
vi.mock("@/hooks/store/use-workspace", () => ({ useWorkspace: () => ({ currentWorkspace: undefined }) }));
vi.mock("@/components/runners/add-runner-modal", () => ({ AddRunnerModal: () => null }));
vi.mock("@/components/core/page-title", () => ({ PageHead: () => null }));
vi.mock("@/components/onboarding/steps/common", () => ({
  CommonOnboardingHeader: ({ title }: { title: string }) => <h1>{title}</h1>,
}));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (value: string) => value }) }));
vi.mock("@karma-team/services", () => ({ RunnerService: vi.fn() }));
vi.mock("@karma-team/propel/toast", () => ({ TOAST_TYPE: { ERROR: "error" }, setToast: vi.fn() }));
vi.mock("@karma-team/propel/button", () => ({
  Button: ({
    children,
    prependIcon: _icon,
    variant: _variant,
    size: _size,
    ...props
  }: ComponentProps<"button"> & { prependIcon?: ReactNode; variant?: string; size?: string }) => (
    <button {...props}>{children}</button>
  ),
  getButtonStyling: () => "",
}));
vi.mock("@karma-team/propel/tabs", () => {
  return {
    Tabs: Object.assign(({ children }: { children: ReactNode }) => <div>{children}</div>, {
      List: ({ children }: { children: ReactNode }) => <div>{children}</div>,
      Trigger: ({ children }: { children: ReactNode }) => <div>{children}</div>,
      Content: ({ children }: { children: ReactNode }) => <div>{children}</div>,
      Indicator: () => null,
    }),
  };
});
vi.mock("@karma-team/ui", () => ({
  AlertModalCore: () => null,
  Badge: ({ children }: { children: ReactNode }) => <span>{children}</span>,
}));

import AiDevMachinesPage from "@/app/(all)/[workspaceSlug]/ai-dev-machines/page";
import { CliInstallStep } from "@/components/onboarding/steps/cli-install/root";

const CONFIG: CliInstallConfig = {
  service_url: "http://172.16.13.81:28000",
  install_sh_url: "http://172.16.13.81:28000/api/cli/install.sh",
  install_ps1_url: "http://172.16.13.81:28000/api/cli/install.ps1",
  update_sh_url: "http://172.16.13.81:28000/api/cli/update.sh",
  windows_msi_url: null,
  available_targets: ["x86_64-unknown-linux-gnu"],
};

describe.each([
  ["onboarding", () => <CliInstallStep handleStepChange={vi.fn()} />],
  ["dev machines", () => <AiDevMachinesPage />],
] as const)("%s CLI installation", (_name, view) => {
  beforeEach(() => {
    configState.data = { ...CONFIG };
    configState.error = undefined;
    configState.mutate.mockReset();
  });

  it("renders service-hosted installers without GitHub links", () => {
    const { container } = render(view());
    expect(container.textContent).toContain(CONFIG.install_sh_url);
    expect(container.textContent).toContain(CONFIG.install_ps1_url);
    expect(container.textContent).not.toContain("github.com");
    expect(screen.queryByRole("link", { name: "Download MSI" })).not.toBeInTheDocument();
  });

  it("does not expose an installer while configuration is loading", () => {
    configState.data = undefined;
    const { container } = render(view());
    expect(screen.getByRole("status")).toHaveTextContent("Loading CLI installation configuration");
    expect(container.querySelector("pre")).not.toBeInTheDocument();
  });

  it("allows retry after the configuration endpoint fails", async () => {
    configState.data = undefined;
    configState.error = new Error("Unavailable");
    const user = userEvent.setup();
    const { container } = render(view());
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load CLI installation configuration");
    expect(container.querySelector("pre")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(configState.mutate).toHaveBeenCalledOnce();
  });
});

it("offers the service-hosted MSI only when the backend supplies its URL", () => {
  configState.data = { ...CONFIG, windows_msi_url: "http://172.16.13.81:28000/api/cli/download/windows.msi" };
  render(<AiDevMachinesPage />);
  expect(screen.getByRole("link", { name: "Download MSI" })).toHaveAttribute("href", configState.data.windows_msi_url);
});
