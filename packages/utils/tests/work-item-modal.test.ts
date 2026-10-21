/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { describe, expect, it } from "vitest";
import { getUpdateFormDataForReset } from "../src/work-item/modal";

describe("work branch when changing the form project", () => {
  it("retains an entered branch while resetting project-specific fields", () => {
    const values = getUpdateFormDataForReset("new-project", {
      name: "Work item",
      state_id: "old-project-state",
      git_work_branch: "feat/pinned",
    });
    expect(values.git_work_branch).toBe("feat/pinned");
    expect(values.project_id).toBe("new-project");
    expect(values.state_id).not.toBe("old-project-state");
  });

  it.each(["", undefined])("keeps the empty default for branch %s", (git_work_branch) => {
    expect(getUpdateFormDataForReset("new-project", { git_work_branch }).git_work_branch).toBe("");
  });
});
