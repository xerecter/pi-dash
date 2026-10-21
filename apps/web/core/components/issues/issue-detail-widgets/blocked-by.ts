/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { TStateGroups } from "@karma-team/types";

const RELEASING_STATE_GROUPS: ReadonlySet<TStateGroups> = new Set(["test", "completed", "cancelled"]);

export function isBlockerReleased(stateGroup: TStateGroups | null | undefined): boolean {
  return stateGroup !== null && stateGroup !== undefined && RELEASING_STATE_GROUPS.has(stateGroup);
}
