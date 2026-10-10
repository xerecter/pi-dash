/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { API_BASE_URL } from "@karma-team/constants";
import { APIService } from "@/services/api.service";

export type CliInstallConfig = {
  service_url: string;
  install_sh_url: string;
  install_ps1_url: string;
  windows_msi_url: string | null;
  available_targets: string[];
};

export class CliInstallService extends APIService {
  constructor() {
    super(API_BASE_URL);
  }

  async getConfig(): Promise<CliInstallConfig> {
    const response = await this.get("/api/cli/install-config/");
    return response.data;
  }
}
