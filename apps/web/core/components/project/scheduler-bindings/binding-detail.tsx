import { useMemo, useState } from "react";
import { observer } from "mobx-react";
import { Link, useNavigate } from "react-router";
import useSWR from "swr";
import { EUserPermissions, EUserPermissionsLevel } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
import type { ISchedulerBinding } from "@karma-team/services";
import { SchedulerService } from "@karma-team/services";
import type { IAgentRun, IAgentRunPage } from "@karma-team/types";
import { Badge, Button, ToggleSwitch } from "@karma-team/ui";
import { RunStatusBadge } from "@/components/runners/run-status-badge";
import { useMember } from "@/hooks/store/use-member";
import { useUserPermissions } from "@/hooks/store/user";
import { EditSchedulerBindingModal } from "./edit-binding-modal";
import { UninstallSchedulerBindingModal } from "./uninstall-binding-modal";
import { DEFAULT_SCHEDULER_COLOR } from "./constants";
import { humanizeRrule } from "./rrule-text";

const schedulerService = new SchedulerService();

type Props = {
  workspaceSlug: string;
  projectId: string;
  bindingId: string;
};

function formatDuration(startedAt: string | null, endedAt: string | null): string | null {
  if (!startedAt || !endedAt) return null;
  const milliseconds = new Date(endedAt).getTime() - new Date(startedAt).getTime();
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return null;
  const totalSeconds = Math.round(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function runResultExcerpt(run: IAgentRun): string {
  if (run.error) return run.error;
  const summary = run.done_payload?.summary;
  return typeof summary === "string" ? summary : "";
}

/** Configuration and paginated run history for one project installation. */
export const SchedulerBindingDetail = observer(function SchedulerBindingDetail({
  workspaceSlug,
  projectId,
  bindingId,
}: Props) {
  const { allowPermissions } = useUserPermissions();
  const { getUserDetails } = useMember();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const canManage = allowPermissions([EUserPermissions.ADMIN], EUserPermissionsLevel.PROJECT, workspaceSlug, projectId);
  const isWorkspaceAdmin = allowPermissions([EUserPermissions.ADMIN], EUserPermissionsLevel.WORKSPACE, workspaceSlug);

  const {
    data: binding,
    error: bindingError,
    mutate: mutateBinding,
  } = useSWR<ISchedulerBinding>(
    workspaceSlug && projectId && bindingId ? ["scheduler-binding-detail", workspaceSlug, projectId, bindingId] : null,
    () => schedulerService.retrieveBinding(workspaceSlug, projectId, bindingId),
    { shouldRetryOnError: false }
  );
  const [page, setPage] = useState(1);
  const {
    data: runsPage,
    error: runsError,
    mutate: mutateRuns,
  } = useSWR<IAgentRunPage>(
    workspaceSlug && projectId && bindingId
      ? ["scheduler-binding-runs", workspaceSlug, projectId, bindingId, page]
      : null,
    () => schedulerService.listBindingRuns(workspaceSlug, projectId, bindingId, page),
    { refreshInterval: 15_000, shouldRetryOnError: false }
  );

  const [editOpen, setEditOpen] = useState(false);
  const [uninstallOpen, setUninstallOpen] = useState(false);
  const [promptOpen, setPromptOpen] = useState(false);
  const [toggling, setToggling] = useState(false);
  const scheduleText = useMemo(
    () => (binding ? (humanizeRrule(binding.rrule, binding.dtstart) ?? t("Once at dtstart")) : null),
    [binding, t]
  );
  const listPath = `/${workspaceSlug}/projects/${projectId}/schedulers/list`;
  const formatTimestamp = (timestamp: string | null) =>
    timestamp ? new Date(timestamp).toLocaleString() : t("(never)");

  async function handleToggle(next: boolean) {
    if (!binding || toggling) return;
    setToggling(true);
    try {
      const updated = await schedulerService.updateBinding(workspaceSlug, projectId, binding.id, { enabled: next });
      await mutateBinding(updated, { revalidate: false });
    } catch (error: unknown) {
      const detail = error as { error?: string } | null;
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Something went wrong"),
        message: detail?.error ?? t("Could not update the install."),
      });
    } finally {
      setToggling(false);
    }
  }

  if (bindingError) {
    return (
      <div className="flex flex-col gap-3 p-6">
        <p className="text-13 text-secondary">
          {t("This scheduler install is not available. It may have been uninstalled.")}
        </p>
        <Link to={listPath} className="text-13 font-medium text-primary hover:underline">
          {t("Back to schedulers")}
        </Link>
      </div>
    );
  }
  if (!binding) return <div className="p-6 text-13 text-secondary">{t("Loading…")}</div>;

  const actorName = binding.actor ? (getUserDetails(binding.actor)?.display_name ?? binding.actor) : null;
  const runs = runsPage?.results ?? [];
  const totalPages = runsPage?.total_pages ?? 1;
  const totalCount = runsPage?.total_count ?? 0;

  return (
    <div className="flex flex-col gap-6 p-6">
      <div>
        <Link to={listPath} className="text-13 text-secondary hover:text-primary hover:underline">
          {t("Back to schedulers")}
        </Link>
      </div>

      <header className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <span
              className="inline-block h-4 w-4 flex-shrink-0 rounded-sm"
              style={{ backgroundColor: binding.scheduler_color || DEFAULT_SCHEDULER_COLOR }}
              aria-hidden="true"
            />
            <h1 className="text-16 font-semibold text-primary">{binding.scheduler_name}</h1>
            {binding.scheduler_source && (
              <Badge variant="accent-neutral" size="sm">
                {binding.scheduler_source}
              </Badge>
            )}
            {binding.scheduler_is_enabled === false && (
              <Badge variant="accent-warning" size="sm">
                {t("Disabled for the whole workspace")}
              </Badge>
            )}
          </div>
          <code className="mt-1 block text-12 text-secondary">{binding.scheduler_slug}</code>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {isWorkspaceAdmin && (
            <Link to={`/${workspaceSlug}/schedulers`} className="text-13 text-secondary hover:text-primary">
              {t("View workspace definition")}
            </Link>
          )}
          {canManage && (
            <>
              <Button variant="neutral-primary" size="sm" onClick={() => setEditOpen(true)}>
                {t("Edit")}
              </Button>
              <Button variant="tertiary-danger" size="sm" onClick={() => setUninstallOpen(true)}>
                {t("Uninstall")}
              </Button>
            </>
          )}
        </div>
      </header>

      {binding.last_error && (
        <div className="rounded-md border border-danger-subtle bg-danger-subtle/20 p-3 text-13">
          <div className="font-medium text-danger-primary">{t("Last error")}</div>
          <pre className="mt-1 text-11 whitespace-pre-wrap text-primary">{binding.last_error}</pre>
        </div>
      )}

      <section className="rounded-md border border-subtle p-4">
        <h2 className="text-14 font-medium text-primary">{t("Configuration")}</h2>
        <dl className="mt-3 grid grid-cols-1 gap-x-8 gap-y-3 text-13 md:grid-cols-2">
          <ConfigRow label={t("Schedule")} value={scheduleText ?? ""} title={binding.rrule || undefined} />
          <ConfigRow label={t("Starts at")} value={formatTimestamp(binding.dtstart)} />
          <ConfigRow label={t("Time zone")} value={binding.tzid || "UTC"} />
          <ConfigRow label={t("Next run")} value={formatTimestamp(binding.next_run_at)} />
          <ConfigRow label={t("Outcome mode")} value={binding.outcome_mode} />
          <ConfigRow label={t("Pod")} value={binding.pod_name ?? t("(default pod)")} />
          {binding.rdates.length > 0 && (
            <ConfigRow
              label={t("Extra dates")}
              value={binding.rdates.map((date) => new Date(date).toLocaleString()).join(", ")}
            />
          )}
          {binding.exdates.length > 0 && (
            <ConfigRow
              label={t("Skipped dates")}
              value={binding.exdates.map((date) => new Date(date).toLocaleString()).join(", ")}
            />
          )}
          {actorName && <ConfigRow label={t("Installed by")} value={actorName} />}
          <div>
            <dt className="text-12 text-tertiary uppercase">{t("Enabled")}</dt>
            <dd className="mt-1 flex items-center gap-2">
              <ToggleSwitch
                value={binding.enabled}
                onChange={handleToggle}
                disabled={toggling || !canManage}
                aria-label={binding.enabled ? t("Disable scheduler") : t("Enable scheduler")}
              />
              <span className="text-12 text-secondary">{binding.enabled ? t("Enabled") : t("Disabled")}</span>
            </dd>
          </div>
        </dl>
        {binding.extra_context && (
          <div className="mt-4">
            <div className="text-12 text-tertiary uppercase">{t("Project context")}</div>
            <pre className="mt-1 rounded bg-layer-1 p-2 text-11 whitespace-pre-wrap">{binding.extra_context}</pre>
          </div>
        )}
        {typeof binding.resolved_prompt === "string" && (
          <div className="mt-4">
            <button
              type="button"
              className="text-13 font-medium text-primary hover:underline"
              onClick={() => setPromptOpen((open) => !open)}
            >
              {promptOpen ? t("Hide resolved prompt") : t("Show resolved prompt")}
            </button>
            {promptOpen && (
              <pre className="mt-2 rounded bg-layer-1 p-2 text-11 whitespace-pre-wrap">{binding.resolved_prompt}</pre>
            )}
          </div>
        )}
      </section>

      <section className="overflow-hidden rounded-md border border-subtle">
        <div className="flex items-center justify-between border-b border-subtle px-4 py-3">
          <h2 className="text-14 font-medium text-primary">{t("Run history")}</h2>
          <span className="text-12 text-secondary">
            {t("{count} runs", { count: binding.run_count ?? totalCount })}
          </span>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[760px] text-13">
            <thead className="bg-layer-1 text-left text-secondary">
              <tr>
                <th className="px-3 py-2">{t("Started")}</th>
                <th className="px-3 py-2">{t("Ended")}</th>
                <th className="px-3 py-2">{t("Status")}</th>
                <th className="px-3 py-2">{t("Duration")}</th>
                <th className="px-3 py-2">{t("Pod")}</th>
                <th className="px-3 py-2">{t("Result")}</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((run) => (
                <tr
                  key={run.id}
                  onClick={() => navigate(`/${workspaceSlug}/projects/${projectId}/runners/runs/${run.id}`)}
                  className="cursor-pointer border-t border-subtle hover:bg-layer-1"
                >
                  <td className="px-3 py-2 whitespace-nowrap">
                    {run.started_at
                      ? new Date(run.started_at).toLocaleString()
                      : t("Queued {ts}", { ts: new Date(run.created_at).toLocaleString() })}
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">{formatTimestamp(run.ended_at)}</td>
                  <td className="px-3 py-2">
                    <RunStatusBadge status={run.status} t={t} />
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">{formatDuration(run.started_at, run.ended_at) ?? "-"}</td>
                  <td className="px-3 py-2 whitespace-nowrap">{run.pod_detail?.name ?? "-"}</td>
                  <td
                    className={`max-w-[280px] truncate px-3 py-2 text-11 ${
                      run.error ? "text-danger-primary" : "text-secondary"
                    }`}
                    title={runResultExcerpt(run) || undefined}
                  >
                    {runResultExcerpt(run)}
                  </td>
                </tr>
              ))}
              {runsError && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-secondary">
                    <div className="flex flex-col items-center gap-2">
                      <span>{t("Could not load run history.")}</span>
                      <Button variant="neutral-primary" size="sm" onClick={() => void mutateRuns()}>
                        {t("Retry")}
                      </Button>
                    </div>
                  </td>
                </tr>
              )}
              {!runsError && runsPage && runs.length === 0 && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-secondary">
                    {binding.enabled
                      ? binding.next_run_at
                        ? t("No runs yet - next run at {ts}", { ts: new Date(binding.next_run_at).toLocaleString() })
                        : t("No runs yet.")
                      : t("Scheduler is disabled - it will not fire until re-enabled.")}
                  </td>
                </tr>
              )}
              {!runsError && !runsPage && (
                <tr>
                  <td colSpan={6} className="px-3 py-8 text-center text-secondary">
                    {t("Loading…")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        {totalCount > 0 && (
          <div className="flex items-center justify-between gap-2 border-t border-subtle bg-layer-1 px-3 py-2 text-11 text-secondary">
            <span>{t("Page {page} of {total}", { page, total: totalPages })}</span>
            <div className="flex items-center gap-2">
              <Button variant="neutral-primary" size="sm" disabled={page <= 1} onClick={() => setPage(page - 1)}>
                {t("Previous")}
              </Button>
              <Button
                variant="neutral-primary"
                size="sm"
                disabled={page >= totalPages}
                onClick={() => setPage(page + 1)}
              >
                {t("Next")}
              </Button>
            </div>
          </div>
        )}
      </section>

      <EditSchedulerBindingModal
        isOpen={editOpen}
        onClose={() => setEditOpen(false)}
        workspaceSlug={workspaceSlug}
        projectId={projectId}
        binding={binding}
        onUpdated={() => mutateBinding()}
      />
      <UninstallSchedulerBindingModal
        isOpen={uninstallOpen}
        onClose={() => setUninstallOpen(false)}
        workspaceSlug={workspaceSlug}
        projectId={projectId}
        binding={uninstallOpen ? binding : null}
        onUninstalled={() => navigate(listPath)}
      />
    </div>
  );
});

function ConfigRow({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div>
      <dt className="text-12 text-tertiary uppercase">{label}</dt>
      <dd className="mt-1 text-13 text-primary" title={title}>
        {value}
      </dd>
    </div>
  );
}
