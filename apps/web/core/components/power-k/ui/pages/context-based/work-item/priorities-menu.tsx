/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { Command } from "cmdk";
import { observer } from "mobx-react";
// karma-team imports
import { ISSUE_PRIORITIES } from "@karma-team/constants";
import { PriorityIcon } from "@karma-team/propel/icons";
import type { TIssue, TIssuePriorities } from "@karma-team/types";
// local imports
import { PowerKModalCommandItem } from "../../../modal/command-item";

type Props = {
  handleSelect: (priority: TIssuePriorities) => void;
  workItemDetails: TIssue;
};

export const PowerKWorkItemPrioritiesMenu = observer(function PowerKWorkItemPrioritiesMenu(props: Props) {
  const { handleSelect, workItemDetails } = props;

  return (
    <Command.Group>
      {ISSUE_PRIORITIES.map((priority) => (
        <PowerKModalCommandItem
          key={priority.key}
          iconNode={<PriorityIcon priority={priority.key} />}
          label={priority.title}
          isSelected={priority.key === workItemDetails.priority}
          onSelect={() => handleSelect(priority.key)}
        />
      ))}
    </Command.Group>
  );
});
