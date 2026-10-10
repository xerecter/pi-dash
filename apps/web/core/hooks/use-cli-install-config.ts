/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import useSWR from "swr";
import { CliInstallService } from "@/services/cli-install.service";

const service = new CliInstallService();

export function useCliInstallConfig(enabled = true) {
  return useSWR(enabled ? "cli-install-config" : null, () => service.getConfig());
}
