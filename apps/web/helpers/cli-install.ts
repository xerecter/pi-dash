/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { CliInstallConfig } from "@/services/cli-install.service";

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function powershellQuote(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

export function getCliInstallCommands(config: CliInstallConfig) {
  const powershell = `irm ${powershellQuote(config.install_ps1_url)} | iex`;
  // Encode only when cmd.exe could interpret characters inside the URL.
  const cmd = /[%"\r\n]/.test(powershell)
    ? `powershell -NoProfile -EncodedCommand ${btoa(
        powershell
          .split("")
          .map((character) => String.fromCharCode(character.charCodeAt(0) & 0xff, character.charCodeAt(0) >> 8))
          .join("")
      )}`
    : `powershell -NoProfile -Command "${powershell}"`;

  return {
    unix: `curl --proto '=http,https' --tlsv1.2 -fsSL ${shellQuote(config.install_sh_url)} | sh`,
    powershell,
    cmd,
    msi: config.windows_msi_url
      ? `$ProgressPreference = "SilentlyContinue"
$msi = Join-Path $env:TEMP "karma-team-x86_64-pc-windows-msvc.msi"
Invoke-WebRequest -Uri ${powershellQuote(config.windows_msi_url)} -OutFile $msi
Start-Process msiexec.exe -Wait -ArgumentList "/i \`"$msi\`""`
      : null,
  };
}
