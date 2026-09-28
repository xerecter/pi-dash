/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import React from "react";
import { observer } from "mobx-react";
// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { PlusIcon, WorkItemsIcon } from "@karma-team/propel/icons";
import type { TIssue, TIssueServiceType } from "@karma-team/types";
import { CustomMenu } from "@karma-team/ui";
// hooks
import { useIssueDetail } from "@/hooks/store/use-issue-detail";

type Props = {
  issueId: string;
  customButton?: React.ReactNode;
  disabled?: boolean;
  issueServiceType: TIssueServiceType;
};

export const SubIssuesActionButton = observer(function SubIssuesActionButton(props: Props) {
  const { issueId, customButton, disabled = false, issueServiceType } = props;
  // translation
  const { t } = useTranslation();
  // store hooks
  const {
    issue: { getIssueById },
    toggleCreateIssueModal,
    toggleSubIssuesModal,
    setIssueCrudOperationState,
    issueCrudOperationState,
  } = useIssueDetail(issueServiceType);

  // derived values
  const issue = getIssueById(issueId);

  if (!issue) return <></>;

  // handlers
  const handleIssueCrudState = (
    key: "create" | "existing",
    _parentIssueId: string | null,
    issue: TIssue | null = null
  ) => {
    setIssueCrudOperationState({
      ...issueCrudOperationState,
      [key]: {
        toggle: !issueCrudOperationState[key].toggle,
        parentIssueId: _parentIssueId,
        issue: issue,
      },
    });
  };

  const handleCreateNew = () => {
    handleIssueCrudState("create", issueId, null);
    toggleCreateIssueModal(true);
  };

  const handleAddExisting = () => {
    handleIssueCrudState("existing", issueId, null);
    toggleSubIssuesModal(issue.id);
  };

  // options
  const optionItems = [
    {
      i18n_label: "Create new",
      icon: <PlusIcon className="h-3 w-3" />,
      onClick: handleCreateNew,
    },
    {
      i18n_label: "Add existing",
      icon: <WorkItemsIcon className="h-3 w-3" />,
      onClick: handleAddExisting,
    },
  ];

  // button element
  const customButtonElement = customButton ? <>{customButton}</> : <PlusIcon className="h-4 w-4" />;

  return (
    <CustomMenu customButton={customButtonElement} placement="bottom-start" disabled={disabled} closeOnSelect>
      {optionItems.map((item, index) => (
        <CustomMenu.MenuItem
          key={index}
          onClick={() => {
            item.onClick();
          }}
        >
          <div className="flex items-center gap-2">
            {item.icon}
            <span>{t(item.i18n_label)}</span>
          </div>
        </CustomMenu.MenuItem>
      ))}
    </CustomMenu>
  );
});
