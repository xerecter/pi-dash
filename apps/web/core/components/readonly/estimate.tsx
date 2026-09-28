/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useEffect } from "react";
import { observer } from "mobx-react";
import { useTranslation } from "@karma-team/i18n";
import { EstimatePropertyIcon } from "@karma-team/propel/icons";
import { EEstimateSystem } from "@karma-team/types";
import { cn, convertMinutesToHoursMinutesString } from "@karma-team/utils";
// hooks
import { useProjectEstimates } from "@/hooks/store/estimates";
import { useEstimate } from "@/hooks/store/estimates/use-estimate";

export type TReadonlyEstimateProps = {
  className?: string;
  hideIcon?: boolean;
  value: string | undefined | null;
  placeholder?: string;
  projectId: string | undefined;
  workspaceSlug: string;
};

export const ReadonlyEstimate = observer(function ReadonlyEstimate(props: TReadonlyEstimateProps) {
  const { className, hideIcon = false, value, placeholder, projectId, workspaceSlug } = props;

  const { t } = useTranslation();
  const { currentActiveEstimateIdByProjectId, getEstimateById, getProjectEstimates } = useProjectEstimates();

  const currentActiveEstimateId = projectId ? currentActiveEstimateIdByProjectId(projectId) : undefined;
  const currentActiveEstimate = currentActiveEstimateId ? getEstimateById(currentActiveEstimateId) : undefined;
  const { estimatePointById } = useEstimate(currentActiveEstimateId);

  const estimatePoint = value ? estimatePointById(value) : null;

  const displayValue = estimatePoint
    ? currentActiveEstimate?.type === EEstimateSystem.TIME
      ? convertMinutesToHoursMinutesString(Number(estimatePoint.value))
      : estimatePoint.value
    : null;

  useEffect(() => {
    if (projectId) {
      getProjectEstimates(workspaceSlug, projectId);
    }
  }, [projectId, workspaceSlug]);

  return (
    <div className={cn("flex items-center gap-1 text-body-xs-regular", className)}>
      {!hideIcon && <EstimatePropertyIcon className="size-4 flex-shrink-0" />}
      <span className="flex-grow truncate">{displayValue ?? placeholder ?? t("None")}</span>
    </div>
  );
});
