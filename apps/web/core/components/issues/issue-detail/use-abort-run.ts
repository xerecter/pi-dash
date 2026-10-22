/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useCallback, useRef, useState } from "react";
import { useTranslation } from "@karma-team/i18n";
import { TOAST_TYPE, setToast } from "@karma-team/propel/toast";
// services
import { AgentRunService } from "@/services/runner";
import type { TAgentRun } from "@/services/runner/agent-run.service";

const agentRunService = new AgentRunService();

/**
 * Drives the "Abort run" affordance on the issue AgentRun card: signals the
 * associated runner to stop the active run. The run remains active in
 * ``cancel_requested`` until the runner confirms its agent process stopped,
 * then becomes ``cancelled``. Returns the acknowledged state so the card
 * can reflect the request before its next poll.
 */
export function useAbortRun() {
  const { t } = useTranslation();
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const submitting = useRef(false);

  const abortRun = useCallback(
    async (runId: string): Promise<TAgentRun | null> => {
      if (submitting.current) return null;
      submitting.current = true;
      setIsSubmitting(true);
      setErrorMessage(null);
      try {
        const run = await agentRunService.abortRun(runId, "user");
        setToast({
          type: TOAST_TYPE.SUCCESS,
          title: run.status === "cancelled" ? t("Run cancelled") : t("Abort requested"),
          message:
            run.status === "cancelled"
              ? t("The run has stopped.")
              : t("Waiting for the runner to confirm cancellation."),
        });
        return run;
      } catch (error: unknown) {
        const detail = error as { error?: unknown; detail?: unknown };
        const message =
          typeof detail?.error === "string"
            ? detail.error
            : typeof detail?.detail === "string"
              ? detail.detail
              : t("Could not abort this run. Please try again.");
        setErrorMessage(message);
        setToast({
          type: TOAST_TYPE.ERROR,
          title: t("Failed to abort run"),
          message,
        });
        return null;
      } finally {
        submitting.current = false;
        setIsSubmitting(false);
      }
    },
    [t]
  );

  return { abortRun, isSubmitting, errorMessage };
}
