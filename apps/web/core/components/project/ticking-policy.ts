/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { IProject } from "@karma-team/types";

export const TICKING_CADENCE_OPTIONS = [
  { seconds: 1800, i18n_label: "30 minutes" },
  { seconds: 3600, i18n_label: "1 hour" },
  { seconds: 10800, i18n_label: "3 hours" },
  { seconds: 21600, i18n_label: "6 hours" },
  { seconds: 43200, i18n_label: "12 hours" },
  { seconds: 86400, i18n_label: "24 hours" },
] as const;

export const DEFAULT_CADENCE_SECONDS = 10800;
export const DEFAULT_MAX_TICKS = 10;
export const INFINITE_MAX_TICKS = -1;

type TickingPolicyFormValues = Pick<IProject, "agent_ticking_enabled" | "agent_default_interval_seconds"> & {
  agent_default_max_ticks?: number | string | null;
};

export type TickingPolicyDirtyFields = Partial<
  Record<"agent_ticking_enabled" | "agent_default_max_ticks" | "agent_default_interval_seconds", boolean>
>;

export function cappedBudgetOf(project: Pick<IProject, "agent_default_max_ticks">): number {
  return project.agent_default_max_ticks && project.agent_default_max_ticks > 0
    ? project.agent_default_max_ticks
    : DEFAULT_MAX_TICKS;
}

export function formatCadenceSeconds(seconds: number): string {
  if (seconds % 3600 === 0) return `${seconds / 3600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
}

export function isValidTickBudget(value: unknown): boolean {
  if (value === undefined || value === null || String(value).trim() === "") return true;
  const parsed = Number(value);
  return parsed === INFINITE_MAX_TICKS || (Number.isInteger(parsed) && parsed >= 1);
}

export function buildTickingPolicyPayload(
  formData: TickingPolicyFormValues,
  dirtyFields: TickingPolicyDirtyFields
): Partial<IProject> {
  const payload: Partial<IProject> = {};

  if (dirtyFields.agent_ticking_enabled) {
    payload.agent_ticking_enabled = formData.agent_ticking_enabled ?? true;
  }

  if (dirtyFields.agent_default_max_ticks) {
    const rawBudget = formData.agent_default_max_ticks;
    payload.agent_default_max_ticks =
      rawBudget === undefined || rawBudget === null || String(rawBudget).trim() === ""
        ? DEFAULT_MAX_TICKS
        : Number(rawBudget);
  }

  if (dirtyFields.agent_default_interval_seconds) {
    const cadenceSeconds = Number(formData.agent_default_interval_seconds ?? DEFAULT_CADENCE_SECONDS);
    payload.agent_default_interval_seconds = cadenceSeconds;
    payload.agent_review_default_interval_seconds = cadenceSeconds;
    payload.agent_test_default_interval_seconds = cadenceSeconds;
  }

  return payload;
}
