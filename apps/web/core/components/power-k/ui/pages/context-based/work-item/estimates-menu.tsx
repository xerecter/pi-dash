/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { Command } from "cmdk";
import { observer } from "mobx-react";
import { Triangle } from "lucide-react";
// karma-team types
import { useTranslation } from "@karma-team/i18n";
import { EEstimateSystem } from "@karma-team/types";
import type { TIssue } from "@karma-team/types";
import { Spinner } from "@karma-team/ui";
import { convertMinutesToHoursMinutesString } from "@karma-team/utils";
// hooks
import { useEstimate, useProjectEstimates } from "@/hooks/store/estimates";
// local imports
import { PowerKModalCommandItem } from "../../../modal/command-item";

type Props = {
  handleSelect: (estimatePointId: string | null) => void;
  workItemDetails: TIssue;
};

export const PowerKWorkItemEstimatesMenu = observer(function PowerKWorkItemEstimatesMenu(props: Props) {
  const { handleSelect, workItemDetails } = props;
  // store hooks
  const { currentActiveEstimateIdByProjectId, getEstimateById } = useProjectEstimates();
  const currentActiveEstimateId = workItemDetails.project_id
    ? currentActiveEstimateIdByProjectId(workItemDetails.project_id)
    : undefined;
  const { estimatePointIds, estimatePointById } = useEstimate(currentActiveEstimateId);
  // derived values
  const currentActiveEstimate = currentActiveEstimateId ? getEstimateById(currentActiveEstimateId) : undefined;
  // translation
  const { t } = useTranslation();

  if (!estimatePointIds) return <Spinner />;

  return (
    <Command.Group>
      <PowerKModalCommandItem
        icon={Triangle}
        label={t("No estimate")}
        isSelected={workItemDetails.estimate_point === null}
        onSelect={() => handleSelect(null)}
      />
      {estimatePointIds.length > 0 ? (
        estimatePointIds.map((estimatePointId) => {
          const estimatePoint = estimatePointById(estimatePointId);
          if (!estimatePoint) return null;

          return (
            <PowerKModalCommandItem
              key={estimatePoint.id}
              icon={Triangle}
              label={
                currentActiveEstimate?.type === EEstimateSystem.TIME
                  ? convertMinutesToHoursMinutesString(Number(estimatePoint.value))
                  : estimatePoint.value
              }
              isSelected={workItemDetails.estimate_point === estimatePoint.id}
              onSelect={() => handleSelect(estimatePoint.id ?? null)}
            />
          );
        })
      ) : (
        <div className="text-center">No estimate found</div>
      )}
    </Command.Group>
  );
});
