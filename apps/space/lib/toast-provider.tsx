/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useTheme } from "next-themes";
// karma-team imports
import { Toast } from "@karma-team/propel/toast";
import { resolveGeneralTheme } from "@karma-team/utils";

export function ToastProvider({ children }: { children: React.ReactNode }) {
  // themes
  const { resolvedTheme } = useTheme();

  return (
    <>
      <Toast theme={resolveGeneralTheme(resolvedTheme)} />
      {children}
    </>
  );
}
