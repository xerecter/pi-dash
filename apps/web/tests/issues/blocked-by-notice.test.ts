/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { describe, expect, it } from "vitest";
import type { TStateGroups } from "@karma-team/types";
import { isBlockerReleased } from "@/components/issues/issue-detail-widgets/blocked-by";

describe("blocked-by release groups", () => {
  it.each<TStateGroups>(["test", "completed", "cancelled"])("releases a blocker in %s", (group) => {
    expect(isBlockerReleased(group)).toBe(true);
  });

  it.each<Array<TStateGroups | null | undefined>>([
    ["backlog"],
    ["unstarted"],
    ["started"],
    ["review"],
    [null],
    [undefined],
  ])("keeps a blocker unresolved in %s", (group) => {
    expect(isBlockerReleased(group)).toBe(false);
  });
});
