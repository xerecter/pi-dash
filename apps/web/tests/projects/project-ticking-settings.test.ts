/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { describe, expect, it } from "vitest";
import {
  buildTickingPolicyPayload,
  cappedBudgetOf,
  DEFAULT_MAX_TICKS,
  formatCadenceSeconds,
  INFINITE_MAX_TICKS,
  isValidTickBudget,
} from "@/components/project/ticking-policy";

describe("project ticking settings", () => {
  const formValues = {
    agent_ticking_enabled: true,
    agent_default_max_ticks: 10,
    agent_default_interval_seconds: 21600,
  };

  it("does not overwrite any ticking policy field on an unrelated save", () => {
    expect(buildTickingPolicyPayload(formValues, {})).toEqual({});
  });

  it("writes only the toggle when only the toggle changed", () => {
    const payload = buildTickingPolicyPayload(
      { ...formValues, agent_ticking_enabled: false },
      { agent_ticking_enabled: true }
    );

    expect(payload).toEqual({ agent_ticking_enabled: false });
  });

  it("writes one changed cadence to all three ticking stages", () => {
    const payload = buildTickingPolicyPayload(formValues, { agent_default_interval_seconds: true });

    expect(payload).toEqual({
      agent_default_interval_seconds: 21600,
      agent_review_default_interval_seconds: 21600,
      agent_test_default_interval_seconds: 21600,
    });
  });

  it("writes only the normalized budget when the number input is cleared", () => {
    const payload = buildTickingPolicyPayload(
      { ...formValues, agent_default_max_ticks: "" },
      { agent_default_max_ticks: true }
    );

    expect(payload).toEqual({ agent_default_max_ticks: DEFAULT_MAX_TICKS });
    expect(isValidTickBudget("")).toBe(true);
  });

  it("preserves the no-cap sentinel", () => {
    const payload = buildTickingPolicyPayload(
      { ...formValues, agent_default_max_ticks: INFINITE_MAX_TICKS },
      { agent_default_max_ticks: true }
    );

    expect(payload.agent_default_max_ticks).toBe(INFINITE_MAX_TICKS);
    expect(isValidTickBudget(INFINITE_MAX_TICKS)).toBe(true);
    expect(isValidTickBudget(0)).toBe(false);
    expect(isValidTickBudget(-2)).toBe(false);
    expect(isValidTickBudget(1.5)).toBe(false);
  });

  it("composes multiple changed fields without including untouched policy", () => {
    const payload = buildTickingPolicyPayload(
      { ...formValues, agent_ticking_enabled: false, agent_default_max_ticks: 20 },
      { agent_ticking_enabled: true, agent_default_max_ticks: true }
    );

    expect(payload).toEqual({
      agent_ticking_enabled: false,
      agent_default_max_ticks: 20,
    });
  });

  it("formats custom cadences and restores a capped budget per project", () => {
    expect(formatCadenceSeconds(28800)).toBe("8h");
    expect(formatCadenceSeconds(5400)).toBe("90m");
    expect(formatCadenceSeconds(75)).toBe("75s");
    expect(cappedBudgetOf({ agent_default_max_ticks: 20 })).toBe(20);
    expect(cappedBudgetOf({ agent_default_max_ticks: INFINITE_MAX_TICKS })).toBe(DEFAULT_MAX_TICKS);
  });
});
