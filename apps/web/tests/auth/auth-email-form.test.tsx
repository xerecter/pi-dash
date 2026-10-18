/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

vi.mock("mobx-react", () => ({
  observer: (component: unknown) => component,
}));

vi.mock("@karma-team/i18n", () => ({
  useTranslation: () => ({ t: (value: string) => value }),
}));

vi.mock("@karma-team/propel/button", () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}));

vi.mock("@karma-team/ui", () => ({
  Input: forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(function Input(props, ref) {
    return <input ref={ref} {...props} />;
  }),
  Spinner: () => <span data-testid="spinner" />,
}));

import { AuthEmailForm } from "@/components/account/auth-forms/email";

describe("AuthEmailForm", () => {
  it("accepts a non-email account", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();

    render(<AuthEmailForm defaultEmail="" onSubmit={onSubmit} />);

    const input = screen.getByRole("textbox", { name: "Email or username" });
    await user.type(input, "xuhaoxuan");
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(onSubmit).toHaveBeenCalledWith({ email: "xuhaoxuan" });
  });

  it("accepts Karma account punctuation without applying email syntax rules", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();

    render(<AuthEmailForm defaultEmail="" onSubmit={onSubmit} />);

    const input = screen.getByRole("textbox", { name: "Email or username" });
    await user.type(input, "team/user+east");
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(onSubmit).toHaveBeenCalledWith({ email: "team/user+east" });
  });

  it("normalizes outer whitespace before submitting a Karma account identifier", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();

    render(<AuthEmailForm defaultEmail="" onSubmit={onSubmit} />);

    const input = screen.getByRole("textbox", { name: "Email or username" });
    await user.type(input, " Account Name ");
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(onSubmit).toHaveBeenCalledWith({ email: "Account Name" });
  });

  it.each(["testfree1", "not-an-email@", "person@example.com"])(
    "accepts registration identifier %s",
    async (identifier) => {
      const onSubmit = vi.fn().mockResolvedValue(undefined);
      const user = userEvent.setup();

      render(<AuthEmailForm defaultEmail="" onSubmit={onSubmit} />);

      const input = screen.getByRole("textbox", { name: "Email or username" });
      await user.type(input, identifier);
      await user.click(screen.getByRole("button", { name: "Continue" }));

      expect(input).toHaveAttribute("type", "text");
      expect(onSubmit).toHaveBeenCalledWith({ email: identifier });
    }
  );

  it("does not submit whitespace as an account", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<AuthEmailForm defaultEmail="" onSubmit={onSubmit} />);

    await user.type(screen.getByRole("textbox"), "   ");

    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
