/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { describe, expect, it } from "vitest";
import { getCliInstallCommands } from "@/helpers/cli-install";
import type { CliInstallConfig } from "@/services/cli-install.service";

const CONFIG: CliInstallConfig = {
  service_url: "http://172.16.13.81:28000",
  install_sh_url: "http://172.16.13.81:28000/api/cli/install.sh",
  install_ps1_url: "http://172.16.13.81:28000/api/cli/install.ps1",
  update_sh_url: "http://172.16.13.81:28000/api/cli/update.sh",
  uninstall_sh_url: "http://172.16.13.81:28000/api/cli/uninstall.sh",
  windows_msi_url: null,
  available_targets: ["x86_64-unknown-linux-gnu"],
};

describe("self-hosted CLI installation commands", () => {
  it("downloads over HTTP or HTTPS from the configured backend", () => {
    const commands = getCliInstallCommands(CONFIG);
    expect(commands.unix).toBe(
      "curl --proto '=http,https' --tlsv1.2 -fsSL 'http://172.16.13.81:28000/api/cli/install.sh' | sh"
    );
    expect(commands.updateUnix).toBe(
      "curl --proto '=http,https' --tlsv1.2 -fsSL 'http://172.16.13.81:28000/api/cli/update.sh' | sh"
    );
    expect(commands.uninstallUnix).toBe(
      "curl --proto '=http,https' --tlsv1.2 -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' | sh"
    );
    expect(commands.powershell).toBe("irm 'http://172.16.13.81:28000/api/cli/install.ps1' | iex");
    expect(commands.cmd).toBe(
      `powershell -NoProfile -Command "irm 'http://172.16.13.81:28000/api/cli/install.ps1' | iex"`
    );
    expect(JSON.stringify(commands)).not.toContain("github.com");
    expect(commands.msi).toBeNull();
  });

  it("handles an HTTPS deployment prefix and quotes shell metacharacters", () => {
    const config = {
      ...CONFIG,
      install_sh_url: "https://example.test/team's/api/cli/install.sh",
      install_ps1_url: "https://example.test/team's/api/cli/install.ps1",
      uninstall_sh_url: "https://example.test/team's/api/cli/uninstall.sh",
    };
    const commands = getCliInstallCommands(config);
    expect(commands.unix).toContain("'https://example.test/team'\\''s/api/cli/install.sh'");
    expect(commands.uninstallUnix).toContain("'https://example.test/team'\\''s/api/cli/uninstall.sh'");
    expect(commands.powershell).toBe("irm 'https://example.test/team''s/api/cli/install.ps1' | iex");
  });

  it("protects encoded URL characters from cmd.exe environment expansion", () => {
    const commands = getCliInstallCommands({
      ...CONFIG,
      install_ps1_url: "https://example.test/team%20files/api/cli/install.ps1",
    });
    expect(commands.cmd).toMatch(/^powershell -NoProfile -EncodedCommand /);
    const encoded = commands.cmd.split(" ").at(-1)!;
    expect(Buffer.from(encoded, "base64").toString("utf16le")).toBe(commands.powershell);
  });

  it("only generates an MSI command when the backend publishes an MSI", () => {
    const commands = getCliInstallCommands({
      ...CONFIG,
      windows_msi_url: "https://example.test/team's/api/cli/download/windows.msi",
    });
    expect(commands.msi).toContain("-Uri 'https://example.test/team''s/api/cli/download/windows.msi'");
    expect(commands.msi).toContain("Start-Process msiexec.exe -Wait");
  });
});
