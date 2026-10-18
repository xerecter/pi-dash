/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EOnboardingSteps } from "@karma-team/types";

const onboarding = vi.hoisted(() => ({
  user: {
    id: "user-1",
    last_login_medium: "karma",
  },
  userProfile: {
    onboarding_step: {
      profile_complete: false,
      workspace_create: false,
      workspace_invite: false,
      workspace_join: false,
    },
  },
  updateUserProfile: vi.fn(),
  finishUserOnboarding: vi.fn(),
  workspaces: {},
  instanceConfig: { is_self_managed: true },
}));

vi.mock("mobx-react", () => ({ observer: (component: unknown) => component }));
vi.mock("@/hooks/store/user", () => ({
  useUser: () => ({ data: onboarding.user }),
  useUserProfile: () => ({
    data: onboarding.userProfile,
    updateUserProfile: onboarding.updateUserProfile,
    finishUserOnboarding: onboarding.finishUserOnboarding,
  }),
}));
vi.mock("@/hooks/store/use-workspace", () => ({
  useWorkspace: () => ({ workspaces: onboarding.workspaces }),
}));
vi.mock("@/hooks/store/use-instance", () => ({
  useInstance: () => ({ config: onboarding.instanceConfig }),
}));
vi.mock("@/components/onboarding/header", () => ({ OnboardingHeader: () => null }));
vi.mock("@/components/onboarding/steps", () => ({
  OnboardingStepRoot: ({
    currentStep,
    handleStepChange,
  }: {
    currentStep: EOnboardingSteps;
    handleStepChange: (step: EOnboardingSteps) => Promise<void>;
  }) => (
    <div>
      <span data-testid="current-step">{currentStep}</span>
      <button type="button" onClick={() => void handleStepChange(currentStep)}>
        Continue
      </button>
    </div>
  ),
}));

import { OnboardingRoot } from "@/components/onboarding/root";

describe("Karma profile onboarding", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    onboarding.user.last_login_medium = "karma";
    onboarding.updateUserProfile.mockResolvedValue(undefined);
    onboarding.finishUserOnboarding.mockResolvedValue(undefined);
  });

  it("skips manual name entry after Karma has synchronised the profile", async () => {
    const user = userEvent.setup();
    render(<OnboardingRoot />);

    expect(screen.getByTestId("current-step")).toHaveTextContent(EOnboardingSteps.CLI_INSTALL);
    await user.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() => {
      expect(screen.getByTestId("current-step")).toHaveTextContent(EOnboardingSteps.WORKSPACE_CREATE_OR_JOIN);
    });
    expect(onboarding.updateUserProfile).toHaveBeenCalledWith({
      onboarding_step: {
        profile_complete: true,
        workspace_create: false,
        workspace_invite: false,
        workspace_join: false,
      },
    });
  });

  it("keeps the profile form for non-Karma accounts", async () => {
    const user = userEvent.setup();
    onboarding.user.last_login_medium = "email";
    render(<OnboardingRoot />);

    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(screen.getByTestId("current-step")).toHaveTextContent(EOnboardingSteps.PROFILE_SETUP);
    expect(onboarding.updateUserProfile).not.toHaveBeenCalled();
  });
});
