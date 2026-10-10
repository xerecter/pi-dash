/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useState } from "react";
import { observer } from "mobx-react";
import { Ban, Check, Copy, Cpu, Download, Laptop, RotateCw, Terminal, Trash2 } from "lucide-react";
import useSWR from "swr";
import { useTranslation } from "@karma-team/i18n";
import { Button, getButtonStyling } from "@karma-team/propel/button";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
import { RunnerService } from "@karma-team/services";
import type { IDevMachine } from "@karma-team/types";
import type { TBadgeVariant } from "@karma-team/ui";
import { AlertModalCore, Badge } from "@karma-team/ui";
import { PageHead } from "@/components/core/page-title";
import { AddRunnerModal } from "@/components/runners/add-runner-modal";
import { CliInstallConfigStatus } from "@/components/runners/cli-install-config-status";
import { getCliInstallCommands } from "@/helpers/cli-install";
import { copyCliCommand } from "@/helpers/copy-cli-command";
import { useWorkspace } from "@/hooks/store/use-workspace";
import { useCliInstallConfig } from "@/hooks/use-cli-install-config";

const service = new RunnerService();

type DevMachineStatus = "active" | "offline" | "registered" | "revoked";

const DEV_MACHINE_STATUS_BADGE_VARIANT: Record<DevMachineStatus, TBadgeVariant> = {
  active: "accent-success",
  offline: "accent-neutral",
  registered: "accent-primary",
  revoked: "accent-warning",
};

const DEV_MACHINE_STATUS_I18N_LABELS: Record<DevMachineStatus, string> = {
  active: "Active",
  offline: "Offline",
  registered: "Registered",
  revoked: "Revoked",
};

function getDevMachineStatus(machine: IDevMachine): DevMachineStatus {
  if (machine.revoked_at) return "revoked";
  if (machine.online_runner_count > 0) return "active";
  if (machine.runner_count > 0) return "offline";
  return "registered";
}

function shortMachineId(id: string): string {
  return id.slice(0, 8);
}

function machineDisplayName(machine: IDevMachine): string {
  return machine.label || machine.host_label || shortMachineId(machine.id);
}

function formatDateTime(value: string | null, fallback: string): string {
  return value ? new Date(value).toLocaleString() : fallback;
}

const AiDevMachinesPage = observer(function AiDevMachinesPage() {
  const { currentWorkspace } = useWorkspace();
  const { t } = useTranslation();
  const { data: installConfig, error: installConfigError, mutate: reloadInstallConfig } = useCliInstallConfig();
  const installCommands = installConfig ? getCliInstallCommands(installConfig) : null;

  const workspaceId = currentWorkspace?.id;
  const workspaceSlug = currentWorkspace?.slug;
  const pageTitle = currentWorkspace?.name
    ? t("{workspace} - AI Dev Machines", { workspace: currentWorkspace.name })
    : t("AI Dev Machines");

  const [addOpen, setAddOpen] = useState(false);
  const [rotateMachine, setRotateMachine] = useState<IDevMachine | null>(null);
  const [revokeMachine, setRevokeMachine] = useState<IDevMachine | null>(null);
  const [deleteMachine, setDeleteMachine] = useState<IDevMachine | null>(null);
  const [rotating, setRotating] = useState(false);
  const [revoking, setRevoking] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const {
    data: devMachines,
    error: devMachinesError,
    mutate: mutateDevMachines,
  } = useSWR<IDevMachine[]>(
    workspaceId ? ["dev-machines", workspaceId] : null,
    () => service.listDevMachines(workspaceId!),
    { refreshInterval: 5_000 }
  );

  async function confirmRotateMachine() {
    if (!rotateMachine || !workspaceId) return;
    setRotating(true);
    try {
      await service.rotateDevMachine(rotateMachine.id, workspaceId);
      setRotateMachine(null);
      mutateDevMachines();
    } catch (e: unknown) {
      const err = e as { error?: string } | null;
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Error!"),
        message: err?.error ?? t("Could not rotate the dev machine token."),
      });
    } finally {
      setRotating(false);
    }
  }

  async function confirmRevokeMachine() {
    if (!revokeMachine || !workspaceId) return;
    setRevoking(true);
    try {
      await service.revokeDevMachine(revokeMachine.id, workspaceId);
      setRevokeMachine(null);
      mutateDevMachines();
    } catch (e: unknown) {
      const err = e as { error?: string } | null;
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Error!"),
        message: err?.error ?? t("Could not revoke the dev machine."),
      });
    } finally {
      setRevoking(false);
    }
  }

  async function confirmDeleteMachine() {
    if (!deleteMachine || !workspaceId) return;
    setDeleting(true);
    try {
      await service.deleteDevMachine(deleteMachine.id, workspaceId);
      setDeleteMachine(null);
      mutateDevMachines();
    } catch (e: unknown) {
      const err = e as { error?: string } | null;
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Error!"),
        message: err?.error ?? t("Could not delete the dev machine."),
      });
    } finally {
      setDeleting(false);
    }
  }

  return (
    <div className="flex flex-col gap-8 p-6">
      <PageHead title={pageTitle} />

      <header>
        <h1 className="text-16 font-semibold text-primary">{t("AI Dev Machines")}</h1>
      </header>

      {/* Intro — what the CLI / daemon / runner each are */}
      <section className="flex flex-col gap-4">
        <h2 className="text-14 font-semibold text-primary">{t("What is the karma-team CLI, daemon, and runner?")}</h2>
        <p className="text-13 text-secondary">
          {t(
            "karma-team hands AI agents (Claude Code, Codex, …) the keys to a real dev machine so they can pick up work items, write code, and open changes. Three pieces work together to make that possible:"
          )}
        </p>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <ConceptCard
            icon={<Terminal className="size-4" />}
            title={t("karma-team CLI")}
            body={t(
              "The command-line tool installed on each dev machine. Handles authentication with the cloud, manages local config (`~/.karma-team/config.toml`), and exposes commands for issues, comments, and runner management (`karma-team auth login`, `karma-team runner add`, `karma-team doctor`, …)."
            )}
          />
          <ConceptCard
            icon={<Cpu className="size-4" />}
            title={t("karma-team daemon")}
            body={t(
              "A long-running background process that maintains the WebSocket session with karma-team cloud, dispatches work to the configured agent, and streams approvals + heartbeats back. One daemon per machine."
            )}
          />
          <ConceptCard
            icon={<Laptop className="size-4" />}
            title={t("AI Agent runner")}
            body={t(
              "A cloud-side row that represents one agent instance bound to a project (and optionally a pod). Running `karma-team runner add` on a logged-in machine creates the row and binds that machine as the host. A machine can host many runners."
            )}
          />
        </div>
      </section>

      <section className="flex flex-col gap-3">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h2 className="text-14 font-semibold text-primary">{t("Dev machines")}</h2>
            <p className="mt-1 text-13 text-secondary">
              {t("Machines that have authenticated with karma-team or host runners for this workspace.")}
            </p>
          </div>
          <Button onClick={() => setAddOpen(true)} disabled={!workspaceId}>
            {t("Add runner")}
          </Button>
        </div>
        <div className="overflow-x-auto rounded-md border border-subtle">
          <table className="w-full text-13">
            <thead className="bg-layer-1 text-left text-secondary">
              <tr>
                <th className="px-3 py-2">{t("Machine")}</th>
                <th className="px-3 py-2">{t("Status")}</th>
                <th className="px-3 py-2">{t("Runners")}</th>
                <th className="px-3 py-2">{t("Last seen")}</th>
                <th className="px-3 py-2">{t("Last heartbeat")}</th>
                <th className="px-3 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {devMachinesError && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-danger-primary">
                    {t("Could not load dev machines.")}
                  </td>
                </tr>
              )}
              {!devMachinesError && !devMachines && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-secondary">
                    {t("Loading dev machines...")}
                  </td>
                </tr>
              )}
              {!devMachinesError &&
                (devMachines ?? []).map((machine) => {
                  const status = getDevMachineStatus(machine);
                  const name = machineDisplayName(machine);
                  const hostLabel = machine.host_label && machine.host_label !== name ? machine.host_label : "";
                  return (
                    <tr key={machine.id} className="border-t border-subtle">
                      <td className="px-3 py-2">
                        <div className="font-medium text-primary">{name}</div>
                        <div className="font-mono text-11 text-secondary">
                          {hostLabel || t("id {id}", { id: shortMachineId(machine.id) })}
                        </div>
                      </td>
                      <td className="px-3 py-2">
                        <Badge variant={DEV_MACHINE_STATUS_BADGE_VARIANT[status]} size="sm">
                          {t(DEV_MACHINE_STATUS_I18N_LABELS[status])}
                        </Badge>
                      </td>
                      <td className="px-3 py-2">
                        {t("{active} active / {total} total", {
                          active: machine.online_runner_count,
                          total: machine.runner_count,
                        })}
                      </td>
                      <td className="px-3 py-2">{formatDateTime(machine.last_seen_at, t("Never"))}</td>
                      <td className="px-3 py-2">{formatDateTime(machine.last_heartbeat_at, t("Never"))}</td>
                      <td className="px-3 py-2 text-right">
                        <div className="flex justify-end gap-2">
                          {!machine.revoked_at && (
                            <>
                              <Button
                                size="sm"
                                variant="secondary"
                                prependIcon={<RotateCw />}
                                onClick={() => setRotateMachine(machine)}
                              >
                                {t("Rotate")}
                              </Button>
                              <Button
                                size="sm"
                                variant="error-outline"
                                prependIcon={<Ban />}
                                onClick={() => setRevokeMachine(machine)}
                              >
                                {t("Revoke")}
                              </Button>
                            </>
                          )}
                          <Button
                            size="sm"
                            variant="error-outline"
                            prependIcon={<Trash2 />}
                            onClick={() => setDeleteMachine(machine)}
                          >
                            {t("Delete")}
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              {!devMachinesError && devMachines && devMachines.length === 0 && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-secondary">
                    {t("No dev machines registered for this workspace yet.")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>

      {/* Install command — copy / paste */}
      <section className="flex flex-col gap-3">
        <h2 className="text-14 font-semibold text-primary">{t("Install the karma-team CLI")}</h2>
        <p className="text-13 text-secondary">
          {t(
            "Run an installer on the machine that will host your AI agent. The installer downloads karma-team from this service, adds it to your PATH, and starts device-code login."
          )}
        </p>

        {installConfig && installCommands ? (
          <>
            <InstallCommand label={t("macOS / Linux")} command={installCommands.unix} />
            <InstallCommand label={t("Windows (PowerShell)")} command={installCommands.powershell} />
            {installConfig.windows_msi_url && installCommands.msi && (
              <InstallCommand
                label={t("Windows (MSI)")}
                command={installCommands.msi}
                href={installConfig.windows_msi_url}
                hrefLabel={t("Download MSI")}
              />
            )}
          </>
        ) : (
          <CliInstallConfigStatus error={installConfigError} onRetry={() => void reloadInstallConfig()} />
        )}

        <p className="text-12 text-secondary">
          {t(
            "Prerequisite: the agent CLI you plan to use (`codex` or `claude`) must already be installed and on PATH. Run `karma-team doctor` after install to verify."
          )}
        </p>
      </section>

      {workspaceId && workspaceSlug && (
        <AddRunnerModal
          isOpen={addOpen}
          onClose={() => setAddOpen(false)}
          workspaceId={workspaceId}
          workspaceSlug={workspaceSlug}
        />
      )}
      <AlertModalCore
        isOpen={!!rotateMachine}
        handleClose={() => (rotating ? null : setRotateMachine(null))}
        handleSubmit={confirmRotateMachine}
        isSubmitting={rotating}
        title={t("Rotate dev machine token?")}
        content={t(
          "The active auth token for this dev machine will be invalidated. Runners on that machine will stop connecting until `karma-team auth login` is run there again."
        )}
        variant="primary"
        primaryButtonText={{ default: t("Rotate"), loading: t("Rotate") }}
      />
      <AlertModalCore
        isOpen={!!revokeMachine}
        handleClose={() => (revoking ? null : setRevokeMachine(null))}
        handleSubmit={confirmRevokeMachine}
        isSubmitting={revoking}
        title={t("Revoke dev machine?")}
        content={t(
          "This permanently revokes the dev machine, invalidates its auth token, and revokes runners hosted on it. Use this when the machine should no longer be trusted."
        )}
        primaryButtonText={{ default: t("Revoke"), loading: t("Revoke") }}
      />
      <AlertModalCore
        isOpen={!!deleteMachine}
        handleClose={() => (deleting ? null : setDeleteMachine(null))}
        handleSubmit={confirmDeleteMachine}
        isSubmitting={deleting}
        title={t("Delete dev machine?")}
        content={t(
          "This removes the dev machine connection entirely: its auth token is invalidated, the runners hosted on it are torn down, and the machine disappears from this list. Deleting a dev machine does not uninstall the AI agent (such as Codex or Claude) on the machine itself."
        )}
        primaryButtonText={{ default: t("Delete"), loading: t("Delete") }}
      />
    </div>
  );
});

type ConceptCardProps = {
  icon: React.ReactNode;
  title: string;
  body: string;
};

function ConceptCard({ icon, title, body }: ConceptCardProps) {
  return (
    <div className="flex flex-col gap-2 rounded-md border border-subtle bg-layer-1 p-3">
      <div className="flex items-center gap-2 text-primary">
        {icon}
        <span className="text-13 font-medium">{title}</span>
      </div>
      <p className="text-12 text-secondary">{body}</p>
    </div>
  );
}

type InstallCommandProps = {
  label: string;
  command: string;
  href?: string;
  hrefLabel?: string;
};

function InstallCommand({ label, command, href, hrefLabel }: InstallCommandProps) {
  const { t } = useTranslation();
  const [justCopied, setJustCopied] = useState(false);

  const copy = async () => {
    try {
      await copyCliCommand(command);
      setJustCopied(true);
      window.setTimeout(() => setJustCopied(false), 2000);
    } catch {
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Error!"),
        message: t("Could not copy to clipboard"),
      });
    }
  };

  return (
    <div className="rounded-md border border-subtle bg-layer-1 p-3">
      <div className="mb-2 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <span className="text-12 font-medium text-secondary uppercase">{label}</span>
        <div className="flex flex-wrap items-center gap-2">
          {href && hrefLabel && (
            <a href={href} target="_blank" rel="noreferrer noopener" className={getButtonStyling("secondary", "sm")}>
              <Download className="size-3.5 shrink-0" />
              {hrefLabel}
            </a>
          )}
          <Button size="sm" variant="secondary" onClick={copy} prependIcon={justCopied ? <Check /> : <Copy />}>
            {justCopied ? t("Copied!") : t("Copy command")}
          </Button>
        </div>
      </div>
      <pre className="font-mono rounded border border-subtle bg-layer-2 p-2 text-11 whitespace-pre-wrap text-primary select-all">
        {command}
      </pre>
    </div>
  );
}

export default AiDevMachinesPage;
