/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef, type InputHTMLAttributes, type ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

const services = vi.hoisted(() => ({
  resetPassword: vi.fn(),
  checkAccount: vi.fn(),
  generateCode: vi.fn(),
  verifyCode: vi.fn(),
  signOut: vi.fn(),
  joinWorkspace: vi.fn(),
}));

vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams() }));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (value: string) => value }) }));
vi.mock("@karma-team/propel/toast", () => ({ setToast: vi.fn(), TOAST_TYPE: { SUCCESS: "success", ERROR: "error" } }));
vi.mock("@karma-team/propel/button", () => ({
  Button: ({
    children,
    disabled,
    loading,
    type,
    onClick,
  }: React.ButtonHTMLAttributes<HTMLButtonElement> & { loading?: boolean }) => (
    <button type={type} disabled={disabled || loading} onClick={onClick}>
      {children}
    </button>
  ),
  getButtonStyling: () => "",
}));
vi.mock("@karma-team/ui", () => ({
  Input: forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { hasError?: boolean }>(function Input(
    { hasError: _hasError, ...props },
    ref
  ) {
    return <input ref={ref} {...props} />;
  }),
  Spinner: () => null,
  ModalCore: ({ children, isOpen }: { children: ReactNode; isOpen: boolean }) =>
    isOpen ? <div>{children}</div> : null,
  EModalPosition: { CENTER: "center" },
  EModalWidth: { XXL: "xxl" },
}));
vi.mock("@karma-team/utils", () => ({ cn: (...values: unknown[]) => values.filter(Boolean).join(" ") }));
vi.mock("@/services/auth.service", () => ({
  AuthService: class {
    requestCSRFToken = () => Promise.resolve({ csrf_token: "csrf-test" });
    sendResetPasswordLink = services.resetPassword;
  },
}));
vi.mock("@/services/user.service", () => ({
  default: {
    checkEmail: services.checkAccount,
    generateEmailCode: services.generateCode,
    verifyEmailCode: services.verifyCode,
  },
}));
vi.mock("@/services/workspace.service", () => ({
  WorkspaceService: class {
    createWorkspaceJoinRequest = services.joinWorkspace;
  },
}));
vi.mock("@/hooks/store/user", () => ({
  useUser: () => ({ signOut: services.signOut }),
  useUserSettings: () => ({ fetchCurrentUserSettings: vi.fn().mockResolvedValue(undefined) }),
}));
vi.mock("@/hooks/store/use-workspace", () => ({
  useWorkspace: () => ({ fetchWorkspaces: vi.fn().mockResolvedValue(undefined) }),
}));
vi.mock("@/helpers/authentication.helper", () => ({ authErrorHandler: () => undefined }));
vi.mock("@/components/onboarding/steps/common", () => ({ CommonOnboardingHeader: () => null }));

import { ForgotPasswordForm } from "@/components/account/auth-forms/forgot-password";
import { ChangeEmailModal } from "@/components/core/modals/change-email-modal";
import { WorkspaceJoinByEmailStep } from "@/components/onboarding/steps/workspace/join-by-email";

beforeEach(() => {
  vi.clearAllMocks();
  services.resetPassword.mockResolvedValue(undefined);
  services.checkAccount.mockResolvedValue({ existing: false });
  services.generateCode.mockResolvedValue(undefined);
  services.verifyCode.mockResolvedValue(undefined);
  services.signOut.mockResolvedValue(undefined);
  services.joinWorkspace.mockResolvedValue({ message: "Request sent" });
});

describe("password recovery account input", () => {
  it.each(["testfree1", "team/user+east", "not-an-email@", "member@example.com"])("submits %s", async (identifier) => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <ForgotPasswordForm />
      </MemoryRouter>
    );
    const input = screen.getByRole("textbox", { name: "Email or username" });
    expect(input).toHaveAttribute("type", "text");
    await user.type(input, ` ${identifier} `);
    await user.click(screen.getByRole("button", { name: "Send reset link" }));
    await waitFor(() => expect(services.resetPassword).toHaveBeenCalledWith({ email: identifier }));
  });

  it("keeps whitespace-only accounts disabled", async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <ForgotPasswordForm />
      </MemoryRouter>
    );
    await user.type(screen.getByRole("textbox"), "   ");
    expect(screen.getByRole("button", { name: "Send reset link" })).toBeDisabled();
    expect(services.resetPassword).not.toHaveBeenCalled();
  });
});

describe("account change", () => {
  it("accepts a username and still requires the verification code", async () => {
    const user = userEvent.setup();
    render(<ChangeEmailModal isOpen onClose={vi.fn()} />);
    const input = screen.getByRole("textbox");
    expect(input).toHaveAttribute("type", "text");
    await user.type(input, " new-account ");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(services.generateCode).toHaveBeenCalledWith({ email: "new-account" }));
    expect(services.checkAccount).toHaveBeenCalledWith("csrf-test", "new-account");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(services.verifyCode).not.toHaveBeenCalled();
    await user.type(screen.getByPlaceholderText("123456"), "123456");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(services.verifyCode).toHaveBeenCalledWith({ email: "new-account", code: "123456" }));
  });
});

describe("request workspace access by admin account", () => {
  it("submits an admin username", async () => {
    const user = userEvent.setup();
    const onRequestSent = vi.fn();
    render(<WorkspaceJoinByEmailStep onRequestSent={onRequestSent} onAlreadyMember={vi.fn()} onBack={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Workspace admin email or username" });
    expect(input).toHaveAttribute("type", "text");
    await user.type(input, " testfree1 ");
    await user.click(screen.getByRole("button", { name: "Send join request" }));
    await waitFor(() => expect(services.joinWorkspace).toHaveBeenCalledWith({ admin_email: "testfree1" }));
    expect(onRequestSent).toHaveBeenCalledWith("testfree1");
  });
});
