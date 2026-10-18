/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef, type ButtonHTMLAttributes, type InputHTMLAttributes } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IWorkspace } from "@karma-team/types";

const services = vi.hoisted(() => ({ invite: vi.fn() }));
const workspace = { id: "workspace-id", slug: "account-workspace" } as IWorkspace;

vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (value: string) => value }) }));
vi.mock("@karma-team/propel/toast", () => ({ setToast: vi.fn(), TOAST_TYPE: { SUCCESS: "success", ERROR: "error" } }));
vi.mock("@karma-team/propel/icons", () => ({
  PlusIcon: () => null,
  CheckIcon: () => null,
  ChevronDownIcon: () => null,
}));
vi.mock("@karma-team/propel/button", () => ({
  Button: ({ children, disabled, type, onClick }: ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button type={type} disabled={disabled} onClick={onClick}>
      {children}
    </button>
  ),
}));
vi.mock("@karma-team/ui", () => ({
  Input: forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(function Input(props, ref) {
    return <input ref={ref} {...props} />;
  }),
  Spinner: () => null,
}));
vi.mock("react-popper", () => ({ usePopper: () => ({ styles: {}, attributes: {} }) }));
vi.mock("@/services/workspace.service", () => ({
  WorkspaceService: class {
    inviteWorkspace = services.invite;
  },
}));
vi.mock("@/hooks/store/use-workspace", () => ({
  useWorkspace: () => ({ workspaces: { "workspace-id": { id: "workspace-id", slug: "account-workspace" } } }),
}));
vi.mock("@/components/onboarding/switch-account-dropdown", () => ({ SwitchAccountDropdown: () => null }));
vi.mock("@/components/onboarding/steps/common", () => ({ CommonOnboardingHeader: () => null }));

import { InviteMembers } from "@/components/onboarding/invite-members";
import { InviteTeamStep } from "@/components/onboarding/steps/team/root";

beforeEach(() => {
  vi.clearAllMocks();
  services.invite.mockResolvedValue(undefined);
});

describe.each(["legacy", "team-step"] as const)("%s onboarding invitations", (surface) => {
  const renderInvitations = () => {
    const finish = vi.fn().mockResolvedValue(undefined);
    render(
      surface === "legacy" ? (
        <InviteMembers finishOnboarding={finish} totalSteps={3} user={undefined} workspace={workspace} />
      ) : (
        <InviteTeamStep handleStepChange={finish} />
      )
    );
    return finish;
  };

  it("submits usernames, trims identifiers and ignores empty rows", async () => {
    const user = userEvent.setup();
    const finish = renderInvitations();
    const inputs = await screen.findAllByPlaceholderText("Email or username");
    await user.type(inputs[0], " testfree1 ");
    await user.type(inputs[1], "   ");
    await user.type(inputs[2], " not-an-email@ ");
    const submit = screen.getByRole("button", { name: "Continue" });
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(submit);
    await waitFor(() =>
      expect(services.invite).toHaveBeenCalledWith("account-workspace", {
        emails: [
          { email: "testfree1", role: 15 },
          { email: "not-an-email@", role: 15 },
        ],
      })
    );
    expect(finish).toHaveBeenCalledOnce();
  });

  it("requires at least one nonblank account and disables after clearing it", async () => {
    const user = userEvent.setup();
    renderInvitations();
    const inputs = await screen.findAllByPlaceholderText("Email or username");
    const submit = screen.getByRole("button", { name: "Continue" });
    expect(submit).toBeDisabled();
    await user.type(inputs[0], "   ");
    expect(submit).toBeDisabled();
    await user.type(inputs[1], "team/user+east");
    await waitFor(() => expect(submit).toBeEnabled());
    await user.clear(inputs[1]);
    expect(submit).toBeDisabled();
    expect(services.invite).not.toHaveBeenCalled();
  });

  it("disables submission after removing the only populated row", async () => {
    const user = userEvent.setup();
    renderInvitations();
    const inputs = await screen.findAllByPlaceholderText("Email or username");
    await user.type(inputs[0], "testfree1");
    const submit = screen.getByRole("button", { name: "Continue" });
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(screen.getAllByRole("button", { name: "Remove account" })[0]);
    expect(screen.getAllByPlaceholderText("Email or username")).toHaveLength(2);
    expect(submit).toBeDisabled();
    expect(services.invite).not.toHaveBeenCalled();
  });
});
