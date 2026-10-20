/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

describe("live environment", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.stubEnv("API_BASE_URL", "http://api.example.test");
    vi.stubEnv("LIVE_SERVER_SECRET_KEY", "test-secret");
    delete process.env.JSON_BODY_LIMIT;
  });

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("allows full editor documents by default", async () => {
    const { env } = await import("../src/env");

    expect(env.JSON_BODY_LIMIT).toBe("20mb");
  });

  it("accepts a deployment-specific JSON body limit", async () => {
    vi.stubEnv("JSON_BODY_LIMIT", "8mb");

    const { env } = await import("../src/env");

    expect(env.JSON_BODY_LIMIT).toBe("8mb");
  });
});
