/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef, type ComponentType } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const auth = vi.hoisted(() => ({ emailCheck: vi.fn(), generateUniqueCode: vi.fn(), params: new URLSearchParams() }));
vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("next/navigation", () => ({ useSearchParams: () => auth.params }));
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
  OAuthOptions: () => null,
}));
vi.mock("@/hooks/store/use-instance", () => ({
  useInstance: () => ({
    config: { is_smtp_configured: true, is_magic_login_enabled: true, is_email_password_enabled: true },
  }),
}));
vi.mock("@/hooks/oauth", () => ({ useOAuthConfig: () => ({ isOAuthEnabled: false, oAuthOptions: [] }) }));
vi.mock("@karma-team/services", () => ({
  SitesAuthService: class {
    emailCheck = auth.emailCheck;
    generateUniqueCode = auth.generateUniqueCode;
  },
  AuthService: class {
    requestCSRFToken = () => Promise.resolve({ csrf_token: "csrf-token" });
  },
}));
vi.mock("../../../space/components/account/terms-and-conditions", () => ({ TermsAndConditions: () => null }));
vi.mock("../../../space/components/account/auth-forms/unique-code", () => ({
  AuthUniqueCodeForm: () => <span>Magic code</span>,
}));

// This suite uses the Space aliases from vitest.space-auth.config.ts.
const { AuthRoot } = await vi.importActual<{ AuthRoot: ComponentType }>("@/components/account/auth-forms/auth-root");

describe("Space Karma sign-in", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    auth.params = new URLSearchParams();
    auth.emailCheck.mockResolvedValue({ existing: false, status: "MAGIC_CODE" });
    auth.generateUniqueCode.mockResolvedValue({ code: "" });
  });

  it.each(["karma-user", "first-login@example.com"])("sends %s directly to Karma password sign-in", async (account) => {
    const user = userEvent.setup();
    render(<AuthRoot />);
    await user.type(screen.getByRole("textbox"), ` ${account} `);
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByPlaceholderText("Enter password").closest("form")).toHaveAttribute(
      "action",
      expect.stringContaining("/auth/spaces/sign-in/")
    );
    expect(screen.getByRole("textbox")).toHaveValue(account);
    expect(auth.emailCheck).not.toHaveBeenCalled();
    expect(auth.generateUniqueCode).not.toHaveBeenCalled();
  });

  it.each(["karma-user", "person@example.com"])(
    "does not offer local magic-code sign-in for Karma account %s",
    async (account) => {
      const user = userEvent.setup();
      render(<AuthRoot />);
      await user.type(screen.getByRole("textbox"), account);
      await user.click(screen.getByRole("button", { name: "Continue" }));
      expect(screen.queryByText("Sign in with unique code")).not.toBeInTheDocument();
    }
  );

  it("rejects whitespace-only usernames", async () => {
    const user = userEvent.setup();
    render(<AuthRoot />);
    await user.type(screen.getByRole("textbox"), "  ");
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });

  it.each(["karma-user", "not-an-email@", "new@example.com"])("registers with identifier %s", async (identifier) => {
    auth.emailCheck.mockResolvedValue({ existing: false, status: "CREDENTIAL" });
    const user = userEvent.setup();
    render(<AuthRoot />);
    await user.click(screen.getByRole("button", { name: "Create account" }));
    await user.type(screen.getByRole("textbox"), identifier);
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(auth.emailCheck).toHaveBeenCalledWith({ email: identifier });
    expect(screen.getByPlaceholderText("Confirm password").closest("form")).toHaveAttribute(
      "action",
      expect.stringContaining("/auth/spaces/sign-up/")
    );
  });

  it("returns authentication failures to the password step for retry", () => {
    auth.params = new URLSearchParams({ email: "karma-user", error_code: "5065" });
    render(<AuthRoot />);
    expect(screen.getByPlaceholderText("Enter password").closest("form")).toHaveAttribute(
      "action",
      expect.stringContaining("/auth/spaces/sign-in/")
    );
    expect(screen.getByRole("textbox")).toHaveValue("karma-user");
  });

  it.each([false, true])("preserves explicit email registration magic-code routing (existing=%s)", async (existing) => {
    auth.emailCheck.mockResolvedValue({ existing, status: "MAGIC_CODE" });
    const user = userEvent.setup();
    render(<AuthRoot />);
    await user.click(screen.getByRole("button", { name: "Create account" }));
    await user.type(screen.getByRole("textbox"), "mail@example.com");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("Magic code")).toBeInTheDocument();
    expect(auth.generateUniqueCode).toHaveBeenCalledWith({ email: "mail@example.com" });
  });
});
