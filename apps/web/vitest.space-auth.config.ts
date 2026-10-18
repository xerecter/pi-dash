/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import path from "path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Reuse the existing React test dependencies for the Space sign-in surface.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "happy-dom",
    globals: true,
    setupFiles: ["./tests/setup.ts"],
    include: ["tests/auth/space-auth-flow.spec.tsx"],
    css: false,
  },
  resolve: {
    alias: {
      "next/link": path.resolve(__dirname, "./app/compat/next/link.tsx"),
      "next/navigation": path.resolve(__dirname, "./app/compat/next/navigation.ts"),
      "@": path.resolve(__dirname, "../space"),
    },
  },
});
