/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

// karma-team imports
import type { TOAuthConfigs } from "@karma-team/types";

export const useExtendedOAuthConfig = (_oauthActionText: string): TOAuthConfigs => ({
  isOAuthEnabled: false,
  oAuthOptions: [],
});
