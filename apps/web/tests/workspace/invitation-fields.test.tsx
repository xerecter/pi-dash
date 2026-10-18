/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { forwardRef } from "react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));

vi.mock("@karma-team/i18n", () => ({
  useTranslation: () => ({ t: (value: string) => value }),
}));

vi.mock("@karma-team/propel/icons", () => ({ CloseIcon: () => <span /> }));

vi.mock("@karma-team/ui", () => ({
  Input: forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { hasError?: boolean }>(function Input(
    { hasError: _hasError, ...props },
    ref
  ) {
    return <input ref={ref} {...props} />;
  }),
  CustomSelect: Object.assign(
    ({ children, value, onChange }: { children: ReactNode; value: number; onChange: (value: number) => void }) => (
      <select aria-label="Role" value={value} onChange={(event) => onChange(Number(event.target.value))}>
        {children}
      </select>
    ),
    {
      Option: ({ children, value }: { children: ReactNode; value: number }) => (
        <option value={value}>{children}</option>
      ),
    }
  ),
}));

vi.mock("@karma-team/utils", () => ({
  cn: (...values: unknown[]) => values.filter(Boolean).join(" "),
}));

vi.mock("@/hooks/store/user", () => ({
  useUserPermissions: () => ({ workspaceInfoBySlug: () => ({ role: 20 }) }),
}));

import { InvitationFields } from "@/components/workspace/invite-modal/fields";
import { useWorkspaceInvitationActions } from "@/hooks/use-workspace-invitation";
import type { InvitationFormValues } from "@/hooks/use-workspace-invitation";

function InvitationForm({ onSubmit }: { onSubmit: (data: InvitationFormValues) => Promise<void> }) {
  const { control, fields, formState, remove, onFormSubmit } = useWorkspaceInvitationActions({
    onSubmit,
    onClose: () => undefined,
  });

  return (
    <form onSubmit={onFormSubmit}>
      <InvitationFields
        workspaceSlug="test-workspace"
        fields={fields}
        control={control}
        formState={formState}
        remove={remove}
      />
      <button type="submit">Invite</button>
    </form>
  );
}

describe("workspace invitation fields", () => {
  it.each(["testfree1", "team/user+east", "not-an-email@", "person@example.com"])(
    "submits %s without account or email format validation",
    async (identifier) => {
      const onSubmit = vi.fn().mockResolvedValue(undefined);
      const user = userEvent.setup();
      render(<InvitationForm onSubmit={onSubmit} />);

      await user.type(screen.getByPlaceholderText("Email or username"), identifier);
      await user.click(screen.getByRole("button", { name: "Invite" }));

      await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ emails: [{ email: identifier, role: 15 }] }));
    }
  );

  it("requires an identifier before submitting an invitation", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<InvitationForm onSubmit={onSubmit} />);

    await user.click(screen.getByRole("button", { name: "Invite" }));

    expect(await screen.findByText("We need an email or username to invite them.")).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("trims the account before submitting", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<InvitationForm onSubmit={onSubmit} />);

    await user.type(screen.getByPlaceholderText("Email or username"), " testfree1 ");
    await user.click(screen.getByRole("button", { name: "Invite" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith({ emails: [{ email: "testfree1", role: 15 }] }));
  });

  it("rejects whitespace-only accounts", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<InvitationForm onSubmit={onSubmit} />);

    await user.type(screen.getByPlaceholderText("Email or username"), "   ");
    await user.click(screen.getByRole("button", { name: "Invite" }));

    expect(await screen.findByText("We need an email or username to invite them.")).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
