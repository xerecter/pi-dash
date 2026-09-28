/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useState } from "react";
import { observer } from "mobx-react";
// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
import type { ISchedulerBinding } from "@karma-team/services";
import { SchedulerService } from "@karma-team/services";
import { AlertModalCore } from "@karma-team/ui";

type Props = {
  isOpen: boolean;
  onClose: () => void;
  workspaceSlug: string;
  projectId: string;
  binding: ISchedulerBinding | null;
  onUninstalled: (bindingId: string) => void;
};

const schedulerService = new SchedulerService();

export const UninstallSchedulerBindingModal = observer(function UninstallSchedulerBindingModal(props: Props) {
  const { isOpen, onClose, workspaceSlug, projectId, binding, onUninstalled } = props;
  const { t } = useTranslation();
  const [submitting, setSubmitting] = useState(false);

  const handleUninstall = async () => {
    if (!binding) return;
    setSubmitting(true);
    try {
      await schedulerService.destroyBinding(workspaceSlug, projectId, binding.id);
      setToast({
        type: TOAST_TYPE.SUCCESS,
        title: t("Scheduler uninstalled"),
        message: t("It will not fire on this project until reinstalled."),
      });
      onUninstalled(binding.id);
      onClose();
    } catch (e: unknown) {
      const err = e as { error?: string } | null;
      setToast({
        type: TOAST_TYPE.ERROR,
        title: t("Something went wrong"),
        message: err?.error ?? t("Could not uninstall the scheduler."),
      });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AlertModalCore
      isOpen={isOpen}
      handleClose={() => (submitting ? null : onClose())}
      handleSubmit={handleUninstall}
      isSubmitting={submitting}
      title={t("Uninstall scheduler?")}
      content={
        <>
          <p>
            {t(
              "The scheduler stops firing on this project. The workspace definition is unaffected and can be re-installed later."
            )}
          </p>
          {binding && <p className="mt-2 font-medium text-primary">{binding.scheduler_name}</p>}
        </>
      }
      primaryButtonText={{
        default: t("Uninstall"),
        loading: t("Uninstall"),
      }}
    />
  );
});
