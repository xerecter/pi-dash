import { useEffect, useMemo, useRef, useState } from "react";
import { observer } from "mobx-react";
import type { SubmitHandler } from "react-hook-form";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
import type { IScheduler, ISchedulerBinding, SchedulerOutcomeMode } from "@karma-team/services";
import { SchedulerService } from "@karma-team/services";
import { EModalPosition, EModalWidth, ModalCore } from "@karma-team/ui";
import { deriveSchedulerSlug } from "@/components/schedulers/definition-helpers";
import { SchedulerDefinitionFields } from "@/components/schedulers/scheduler-definition-fields";
import { BindingOutcomeModeField, DEFAULT_OUTCOME_MODE } from "./binding-outcome-mode-field";
import { BindingPodField } from "./binding-pod-field";
import { BindingScheduleFields } from "./binding-schedule-fields";
import { DEFAULT_TZID, SCHEDULER_COLOR_PALETTE as COLOR_PALETTE } from "./constants";
import { defaultDtstartLocal, localToIsoUTC } from "./datetime-input";

export type NewSchedulerMode = "install" | "create";

interface NewSchedulerFormValues {
  scheduler: string;
  name: string;
  slug: string;
  description: string;
  prompt: string;
  color: string;
  dtstart: string;
  tzid: string;
  rrule: string;
  extra_context: string;
  enabled: boolean;
  outcome_mode: SchedulerOutcomeMode;
  pod: string;
}

type Props = {
  isOpen: boolean;
  onClose: () => void;
  workspaceSlug: string;
  projectId: string;
  availableSchedulers: IScheduler[];
  existingBindings: ISchedulerBinding[];
  canCreateScheduler: boolean;
  onInstalled: (binding: ISchedulerBinding) => void;
  onSchedulersChanged: () => void;
};

const defaultValues = (): NewSchedulerFormValues => ({
  scheduler: "",
  name: "",
  slug: "",
  description: "",
  prompt: "",
  color: COLOR_PALETTE[0],
  dtstart: defaultDtstartLocal(),
  tzid: Intl.DateTimeFormat().resolvedOptions().timeZone || DEFAULT_TZID,
  rrule: "FREQ=DAILY",
  extra_context: "",
  enabled: true,
  outcome_mode: DEFAULT_OUTCOME_MODE,
  pod: "",
});

const schedulerService = new SchedulerService();

function bindingErrorMessage(error: unknown, fallback: string): string {
  const detail = error as {
    error?: string;
    rrule?: string[];
    dtstart?: string[];
    tzid?: string[];
    scheduler?: string[];
    pod?: string[];
  } | null;
  return (
    detail?.error ??
    detail?.rrule?.[0] ??
    detail?.dtstart?.[0] ??
    detail?.tzid?.[0] ??
    detail?.scheduler?.[0] ??
    detail?.pod?.[0] ??
    fallback
  );
}

/** Install a catalog scheduler or create a definition and install it. */
export const NewSchedulerModal = observer(function NewSchedulerModal(props: Props) {
  const {
    isOpen,
    onClose,
    workspaceSlug,
    projectId,
    availableSchedulers,
    existingBindings,
    canCreateScheduler,
    onInstalled,
    onSchedulersChanged,
  } = props;
  const { t } = useTranslation();
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setValue,
    formState: { errors, isSubmitting, dirtyFields },
  } = useForm<NewSchedulerFormValues>({ defaultValues: defaultValues() });

  const [mode, setMode] = useState<NewSchedulerMode>("install");
  const [createdForRetry, setCreatedForRetry] = useState<IScheduler | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const modeRef = useRef<NewSchedulerMode>(mode);
  modeRef.current = mode;

  const schedulerCatalog = useMemo(() => {
    if (!createdForRetry || availableSchedulers.some((scheduler) => scheduler.id === createdForRetry.id)) {
      return availableSchedulers;
    }
    return [createdForRetry, ...availableSchedulers];
  }, [availableSchedulers, createdForRetry]);

  const installable = useMemo(() => {
    const boundIds = new Set(existingBindings.map((binding) => binding.scheduler));
    return schedulerCatalog.filter((scheduler) => scheduler.is_enabled && !boundIds.has(scheduler.id));
  }, [schedulerCatalog, existingBindings]);

  const wasOpen = useRef(false);
  useEffect(() => {
    if (isOpen && !wasOpen.current) {
      const initial = defaultValues();
      reset({ ...initial, scheduler: installable[0]?.id ?? "" });
      setCreatedForRetry(null);
      setInstallError(null);
      setMode(installable.length > 0 || !canCreateScheduler ? "install" : "create");
    }
    wasOpen.current = isOpen;
    // Opening is the reset boundary; catalog revalidation must not erase edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, reset]);

  const watchedScheduler = useWatch({ control, name: "scheduler" }) ?? "";
  useEffect(() => {
    // The catalog can resolve after the modal opens. Fill only an empty
    // selection so revalidation cannot replace a user choice or retry target.
    if (!isOpen || mode !== "install" || watchedScheduler || !installable[0]) return;
    setValue("scheduler", installable[0].id);
  }, [installable, isOpen, mode, setValue, watchedScheduler]);

  const watchedName = useWatch({ control, name: "name" }) ?? "";
  useEffect(() => {
    if (mode !== "create" || dirtyFields.slug) return;
    setValue("slug", deriveSchedulerSlug(watchedName));
  }, [dirtyFields.slug, mode, setValue, watchedName]);

  const watchedDtstart = useWatch({ control, name: "dtstart" }) ?? "";
  const watchedRrule = useWatch({ control, name: "rrule" }) ?? "";

  const handleFormSubmit: SubmitHandler<NewSchedulerFormValues> = async (values) => {
    setInstallError(null);
    const bindingPayload = (schedulerId: string) => ({
      scheduler: schedulerId,
      project: projectId,
      dtstart: localToIsoUTC(values.dtstart),
      tzid: values.tzid.trim() || DEFAULT_TZID,
      rrule: values.rrule.trim(),
      extra_context: values.extra_context.trim(),
      enabled: values.enabled,
      outcome_mode: values.outcome_mode,
      pod: values.pod || null,
    });

    if (mode === "create") {
      let created: IScheduler;
      try {
        created = await schedulerService.createScheduler(workspaceSlug, {
          slug: values.slug.trim(),
          name: values.name.trim(),
          description: values.description.trim(),
          prompt: values.prompt,
          color: values.color,
          is_enabled: true,
        });
      } catch (error: unknown) {
        const detail = error as { error?: string; slug?: string[]; name?: string[]; prompt?: string[] } | null;
        if (detail?.slug?.[0]) {
          setError("slug", { type: "server", message: detail.slug[0] });
          return;
        }
        setToast({
          type: TOAST_TYPE.ERROR,
          title: t("Something went wrong"),
          message: detail?.error ?? detail?.name?.[0] ?? detail?.prompt?.[0] ?? t("Could not create the scheduler."),
        });
        return;
      }

      onSchedulersChanged();
      try {
        const binding = await schedulerService.createBinding(workspaceSlug, projectId, bindingPayload(created.id));
        setToast({
          type: TOAST_TYPE.SUCCESS,
          title: t("Scheduler created and installed"),
          message: t("It will fire on the configured schedule."),
        });
        onInstalled(binding);
        onClose();
      } catch (error: unknown) {
        const detail = bindingErrorMessage(error, t("Could not install the scheduler."));
        // Keep the definition locally even before SWR finishes revalidating,
        // so the retry path cannot lose a successfully created scheduler.
        setCreatedForRetry(created);
        setInstallError(detail);
        setMode("install");
        setValue("scheduler", created.id);
        setToast({
          type: TOAST_TYPE.ERROR,
          title: t("Scheduler created but not installed"),
          message: t("The definition was saved. Review the install settings and retry below."),
        });
      }
      return;
    }

    try {
      const binding = await schedulerService.createBinding(workspaceSlug, projectId, bindingPayload(values.scheduler));
      setToast({
        type: TOAST_TYPE.SUCCESS,
        title: t("Scheduler installed"),
        message: t("It will fire on the configured schedule."),
      });
      onInstalled(binding);
      onClose();
    } catch (error: unknown) {
      const detail = bindingErrorMessage(error, t("Could not install the scheduler."));
      setInstallError(detail);
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Something went wrong"),
        message: detail,
      });
    }
  };

  if (installable.length === 0 && !canCreateScheduler) {
    return (
      <ModalCore isOpen={isOpen} handleClose={onClose} position={EModalPosition.CENTER} width={EModalWidth.XL}>
        <div className="flex flex-col gap-4 p-5">
          <div className="text-18 font-medium text-primary">{t("No schedulers available")}</div>
          <p className="text-13 text-secondary">
            {t(
              "Either every workspace scheduler is already installed on this project, or none are enabled. Ask a workspace admin to add schedulers to the catalog."
            )}
          </p>
          <div className="flex justify-end">
            <Button type="button" variant="secondary" onClick={onClose}>
              {t("Cancel")}
            </Button>
          </div>
        </div>
      </ModalCore>
    );
  }

  const installEmpty = installable.length === 0;
  const submitDisabled = isSubmitting || (mode === "install" && installEmpty);

  return (
    <ModalCore isOpen={isOpen} handleClose={onClose} position={EModalPosition.CENTER} width={EModalWidth.XXL}>
      <form onSubmit={handleSubmit(handleFormSubmit)} className="flex max-h-[90vh] flex-col gap-5 overflow-y-auto p-5">
        <div className="text-18 font-medium text-primary">{t("New Scheduler")}</div>

        {canCreateScheduler && (
          <div role="tablist" className="flex w-fit items-center gap-1 rounded-md bg-layer-1 p-1">
            <ModeTab label={t("Install existing")} active={mode === "install"} onSelect={() => setMode("install")} />
            <ModeTab label={t("Create new")} active={mode === "create"} onSelect={() => setMode("create")} />
          </div>
        )}

        {mode === "install" && createdForRetry && (
          <div
            role="alert"
            className="rounded-md border border-warning-subtle bg-warning-subtle p-3 text-13 text-primary"
          >
            <div className="font-medium">{t("Scheduler definition saved")}</div>
            <p className="mt-1 text-secondary">
              {t("{name} was created, but installation failed. It is selected below so you can retry.", {
                name: createdForRetry.name,
              })}
            </p>
            {installError && <p className="mt-1 text-danger-primary">{installError}</p>}
          </div>
        )}

        {mode === "install" &&
          (installEmpty ? (
            <p className="text-13 text-secondary">
              {t("Every enabled workspace scheduler is already installed on this project. Create a new one instead.")}
            </p>
          ) : (
            <div className="flex flex-col gap-1">
              <label htmlFor="binding-scheduler" className="text-13 font-medium text-primary">
                {t("Scheduler")}
              </label>
              <Controller
                control={control}
                name="scheduler"
                rules={{
                  validate: (value) => modeRef.current !== "install" || Boolean(value) || t("Pick a scheduler."),
                }}
                render={({ field }) => (
                  <select
                    {...field}
                    id="binding-scheduler"
                    className="rounded-md border border-subtle bg-surface-1 px-3 py-2 text-13 text-primary focus:ring-1 focus:ring-accent-strong focus:outline-none"
                  >
                    {installable.map((scheduler) => (
                      <option key={scheduler.id} value={scheduler.id}>
                        {scheduler.name} ({scheduler.slug})
                      </option>
                    ))}
                  </select>
                )}
              />
              <p className="text-12 text-secondary">
                {t("Pick from your workspace's enabled schedulers. Already-installed ones aren't listed.")}
                {!canCreateScheduler && <> {t("Ask a workspace admin to add schedulers to the catalog.")}</>}
              </p>
              {errors.scheduler && <span className="text-12 text-danger-primary">{errors.scheduler.message}</span>}
            </div>
          ))}

        {mode === "create" && (
          <SchedulerDefinitionFields
            control={control}
            errors={errors}
            nameName="name"
            slugName="slug"
            descriptionName="description"
            promptName="prompt"
            colorName="color"
            isActive={() => modeRef.current === "create"}
          />
        )}

        <BindingScheduleFields
          control={control}
          errors={errors}
          dtstartName="dtstart"
          tzidName="tzid"
          rruleName="rrule"
          extraContextName="extra_context"
          enabledName="enabled"
          watchDtstart={watchedDtstart}
          watchRrule={watchedRrule}
        />
        <BindingOutcomeModeField control={control} name="outcome_mode" />
        <BindingPodField control={control} name="pod" projectId={projectId} />

        <div className="flex justify-end gap-2 border-t border-subtle pt-4">
          <Button type="button" variant="secondary" onClick={onClose} disabled={isSubmitting}>
            {t("Cancel")}
          </Button>
          <Button type="submit" loading={isSubmitting} disabled={submitDisabled}>
            {mode === "create"
              ? isSubmitting
                ? t("Creating & installing…")
                : t("Create & install")
              : isSubmitting
                ? t("Installing…")
                : createdForRetry
                  ? t("Retry install")
                  : t("Install")}
          </Button>
        </div>
      </form>
    </ModalCore>
  );
});

function ModeTab({ label, active, onSelect }: { label: string; active: boolean; onSelect: () => void }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onSelect}
      className={`rounded px-3 py-1 text-13 font-medium transition-colors ${
        active ? "shadow-sm bg-surface-1 text-primary" : "text-secondary hover:text-primary"
      }`}
    >
      {label}
    </button>
  );
}
