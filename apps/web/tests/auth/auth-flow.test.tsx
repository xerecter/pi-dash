/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef, useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EAuthModes, EAuthSteps } from "@karma-team/constants";

const auth = vi.hoisted(() => ({ emailCheck: vi.fn(), generateUniqueCode: vi.fn(), push: vi.fn() }));
vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams() }));
vi.mock("next/link", () => ({
  default: ({ children, href }: { children: React.ReactNode; href: string }) => <a href={href}>{children}</a>,
}));
vi.mock("@karma-team/i18n", () => ({ useTranslation: () => ({ t: (value: string) => value }) }));
vi.mock("@karma-team/propel/button", () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}));
vi.mock("@karma-team/ui", () => ({
  Input: forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(function Input(props, ref) {
    return <input ref={ref} {...props} />;
  }),
  Spinner: () => null,
  PasswordStrengthIndicator: () => null,
}));
vi.mock("@/hooks/store/use-instance", () => ({ useInstance: () => ({ config: { is_smtp_configured: true } }) }));
vi.mock("@/hooks/use-app-router", () => ({ useAppRouter: () => ({ push: auth.push }) }));
vi.mock("@/services/auth.service", () => ({
  AuthService: class {
    emailCheck = auth.emailCheck;
    generateUniqueCode = auth.generateUniqueCode;
    requestCSRFToken = () => Promise.resolve({ csrf_token: "csrf-token" });
  },
}));
vi.mock("@/components/account/auth-forms/forgot-password-popover", () => ({ ForgotPasswordPopover: () => null }));
vi.mock("@/components/account/auth-forms/unique-code", () => ({ AuthUniqueCodeForm: () => <span>Magic code</span> }));

import { AuthFormRoot } from "@/components/account/auth-forms/form-root";

function AuthFlow({ mode = EAuthModes.SIGN_IN }: { mode?: EAuthModes }) {
  const [authStep, setAuthStep] = useState(EAuthSteps.EMAIL);
  const [authMode, setAuthMode] = useState(mode);
  const [email, setEmail] = useState("");
  return (
    <AuthFormRoot
      {...{ authStep, authMode, email, setEmail, setAuthMode, setAuthStep }}
      setErrorInfo={vi.fn()}
      currentAuthMode={mode}
    />
  );
}

describe("Karma sign-in flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    auth.emailCheck.mockResolvedValue({ existing: false, status: "CREDENTIAL" });
  });

  it.each(["karma-user", "first-login@example.com"])("signs in %s without a local existence check", async (account) => {
    const user = userEvent.setup();
    render(<AuthFlow />);
    await user.type(screen.getByRole("textbox"), ` ${account} `);
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(screen.getByLabelText("Password")).toBeInTheDocument();
    expect(screen.getByRole("textbox")).toHaveValue(account);
    expect(screen.getByLabelText("Password").closest("form")).toHaveAttribute(
      "action",
      expect.stringContaining("/auth/sign-in/")
    );
    expect(auth.emailCheck).not.toHaveBeenCalled();
    expect(auth.generateUniqueCode).not.toHaveBeenCalled();
  });

  it.each(["karma-user", "person@example.com"])(
    "does not offer local recovery methods for Karma account %s",
    async (account) => {
      const user = userEvent.setup();
      render(<AuthFlow />);
      await user.type(screen.getByRole("textbox"), account);
      await user.click(screen.getByRole("button", { name: "Continue" }));
      expect(screen.queryByText("Sign in with unique code")).not.toBeInTheDocument();
      expect(screen.queryByText("Forgot your password?")).not.toBeInTheDocument();
    }
  );

  it("preserves explicit registration", async () => {
    const user = userEvent.setup();
    render(<AuthFlow mode={EAuthModes.SIGN_UP} />);
    await user.type(screen.getByRole("textbox"), "new@example.com");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(auth.emailCheck).toHaveBeenCalledWith({ email: "new@example.com" });
    expect(screen.getByLabelText("Confirm password")).toBeInTheDocument();
    expect(screen.getByLabelText("Confirm password").closest("form")).toHaveAttribute(
      "action",
      expect.stringContaining("/auth/sign-up/")
    );
  });
});
