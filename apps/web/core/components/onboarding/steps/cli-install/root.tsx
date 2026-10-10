/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useEffect, useState } from "react";
import { AlertTriangle, Check, Copy, Terminal } from "lucide-react";
// karma-team imports
import { Button } from "@karma-team/propel/button";
import { Tabs } from "@karma-team/propel/tabs";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
import { EOnboardingSteps } from "@karma-team/types";
import { CliInstallConfigStatus } from "@/components/runners/cli-install-config-status";
import { getCliInstallCommands } from "@/helpers/cli-install";
import { copyCliCommand } from "@/helpers/copy-cli-command";
// hooks
import { useCliInstallConfig } from "@/hooks/use-cli-install-config";
import { usePlatformOS } from "@/hooks/use-platform-os";
// local components
import { CommonOnboardingHeader } from "../common";

type Props = {
  handleStepChange: (step: EOnboardingSteps, skipInvites?: boolean) => void;
};

type TPlatformKey = "unix" | "windows";

type TInstallCommand = {
  id: string;
  /** Shell the command is meant to be pasted into. */
  shell: string;
  command: string;
  /** Rendered above the block when a platform ships more than one shell. */
  hint?: string;
};

function CommandBlock({ command }: { command: TInstallCommand }) {
  const [hasCopied, setHasCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await copyCliCommand(command.command);
      setHasCopied(true);
      window.setTimeout(() => setHasCopied(false), 2000);
    } catch {
      setToast({
        type: TOAST_TYPE.ERROR,
        title: "Couldn't copy",
        message: "Copying isn't available here. Select the command and copy it manually.",
      });
    }
  };

  return (
    <div className="flex flex-col gap-3">
      {command.hint && <p className="text-body-sm-regular text-tertiary">{command.hint}</p>}
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-body-sm-semibold text-placeholder">
          <Terminal className="size-4" />
          <span>{command.shell}</span>
        </div>
        <Button
          variant="secondary"
          size="lg"
          className="shrink-0"
          onClick={handleCopy}
          prependIcon={hasCopied ? <Check /> : <Copy />}
        >
          {hasCopied ? "Copied" : "Copy command"}
        </Button>
      </div>
      <pre className="max-w-full overflow-x-auto rounded-lg border border-subtle bg-surface-2 p-4 text-left">
        <code className="font-mono text-13 leading-5 break-words whitespace-pre-wrap text-secondary">
          {command.command}
        </code>
      </pre>
    </div>
  );
}

export function CliInstallStep({ handleStepChange }: Props) {
  const { platform } = usePlatformOS();
  const { data: installConfig, error: installConfigError, mutate: reloadInstallConfig } = useCliInstallConfig();
  const installCommands = installConfig ? getCliInstallCommands(installConfig) : null;
  const platforms: Record<TPlatformKey, { label: string; commands: TInstallCommand[] }> = {
    unix: {
      label: "macOS / Linux",
      commands: installCommands ? [{ id: "unix-sh", shell: "Terminal", command: installCommands.unix }] : [],
    },
    windows: {
      label: "Windows",
      commands: installCommands
        ? [
            { id: "windows-powershell", shell: "PowerShell", command: installCommands.powershell },
            {
              id: "windows-cmd",
              shell: "Command Prompt",
              hint: "Using Command Prompt instead?",
              command: installCommands.cmd,
            },
          ]
        : [],
    },
  };
  // Preselect the tab matching the browser's OS, but leave it switchable —
  // people routinely set up a runner on a machine other than the one they
  // are reading this on.
  const [selectedPlatform, setSelectedPlatform] = useState<TPlatformKey>("unix");

  useEffect(() => {
    if (platform === "Windows") setSelectedPlatform("windows");
  }, [platform]);

  const continueToProfile = () => handleStepChange(EOnboardingSteps.CLI_INSTALL);

  return (
    <div className="flex flex-col gap-10">
      <CommonOnboardingHeader
        title="Install the karma-team CLI."
        description="Run this on the machine where your coding agent will execute work."
      />

      <div className="flex gap-2 text-warning-primary">
        <AlertTriangle className="mt-0.5 size-4 shrink-0" />
        <p className="text-body-sm-medium">
          Important: install the karma-team CLI and the Karma agent on the dev machine to make karma-team work end to
          end.
        </p>
      </div>

      {!installConfig ? (
        <CliInstallConfigStatus error={installConfigError} onRetry={() => void reloadInstallConfig()} />
      ) : (
        <Tabs
          value={selectedPlatform}
          onValueChange={(value) => setSelectedPlatform(value as TPlatformKey)}
          className="h-auto gap-6"
        >
          <Tabs.List>
            {Object.entries(platforms).map(([key, { label }]) => (
              <Tabs.Trigger key={key} value={key}>
                {label}
              </Tabs.Trigger>
            ))}
            <Tabs.Indicator />
          </Tabs.List>
          {Object.entries(platforms).map(([key, { commands: platformCommands }]) => (
            <Tabs.Content key={key} value={key} className="flex flex-col gap-6">
              {platformCommands.map((command) => (
                <CommandBlock key={command.id} command={command} />
              ))}
            </Tabs.Content>
          ))}
        </Tabs>
      )}

      <div className="space-y-3">
        <Button variant="ghost" size="xl" className="w-full text-tertiary" onClick={continueToProfile}>
          Skip for now
        </Button>
        <Button variant="primary" size="xl" className="w-full" onClick={continueToProfile}>
          Done, Continue
        </Button>
      </div>
    </div>
  );
}
