/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { Clock3 } from "lucide-react";
import { observer } from "mobx-react";
import { useTranslation } from "@karma-team/i18n";
import { Banner } from "@karma-team/propel/banner";
import type { TIssue, TIssueServiceType } from "@karma-team/types";
// hooks
import { useIssueDetail } from "@/hooks/store/use-issue-detail";
import { useProject } from "@/hooks/store/use-project";
import { useProjectState } from "@/hooks/store/use-project-state";
// local
import { isBlockerReleased } from "./blocked-by";

type Props = {
  issueId: string;
  issueServiceType: TIssueServiceType;
};

export const BlockedByNotice = observer(function BlockedByNotice({ issueId, issueServiceType }: Props) {
  const { t } = useTranslation();
  const {
    issue: { getIssueById },
    relation: { getRelationByIssueIdRelationType },
  } = useIssueDetail(issueServiceType);
  const { getProjectById } = useProject();
  const { getStateById } = useProjectState();

  const blockerIds = getRelationByIssueIdRelationType(issueId, "blocked_by") ?? [];
  const unresolvedBlockers = blockerIds
    .map((blockerId) => getIssueById(blockerId))
    .filter((blocker): blocker is TIssue => {
      if (!blocker) return false;
      const stateGroup = blocker.state__group ?? getStateById(blocker.state_id)?.group;
      return !isBlockerReleased(stateGroup);
    });

  if (unresolvedBlockers.length === 0) return null;

  const blockerLabels = unresolvedBlockers.map((blocker) => {
    const projectIdentifier = blocker.project_id ? getProjectById(blocker.project_id)?.identifier : undefined;
    return projectIdentifier ? `#${projectIdentifier}-${blocker.sequence_id}` : `#${blocker.sequence_id}`;
  });
  const message =
    blockerLabels.length === 1
      ? t("Waiting for {workItem} to enter In Test.", { workItem: blockerLabels[0] })
      : t("Waiting for {workItems} to enter In Test.", { workItems: blockerLabels.join(", ") });

  return <Banner variant="warning" icon={<Clock3 className="size-4" />} title={message} />;
});
