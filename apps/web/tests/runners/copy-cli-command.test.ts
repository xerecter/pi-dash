/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { copyCliCommand } from "@/helpers/copy-cli-command";

describe("CLI command clipboard compatibility", () => {
  afterEach(() => vi.restoreAllMocks());

  it("copies through the clipboard API on secure origins", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator, "clipboard", "get").mockReturnValue({ writeText } as unknown as Clipboard);
    await copyCliCommand("karma-team runner add");
    expect(writeText).toHaveBeenCalledWith("karma-team runner add");
    expect(document.querySelector("textarea")).not.toBeInTheDocument();
  });

  it.each(["absent", "rejected"])("falls back when the clipboard API is %s", async (state) => {
    const clipboard =
      state === "absent"
        ? undefined
        : { writeText: vi.fn().mockRejectedValue(new Error("Clipboard permission denied")) };
    vi.spyOn(navigator, "clipboard", "get").mockReturnValue(clipboard as unknown as Clipboard);
    const execCommand = vi.fn(() => {
      expect(document.querySelector("textarea")?.value).toBe("curl 'http://172.16.13.81/install.sh' | sh");
      return true;
    });
    Object.defineProperty(document, "execCommand", { value: execCommand, configurable: true });
    await copyCliCommand("curl 'http://172.16.13.81/install.sh' | sh");
    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(document.querySelector("textarea")).not.toBeInTheDocument();
  });
});
