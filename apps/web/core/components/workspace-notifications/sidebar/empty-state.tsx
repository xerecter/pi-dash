/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
// karma-team imports
import { ENotificationTab } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { EmptyStateCompact } from "@karma-team/propel/empty-state";

type TNotificationEmptyStateProps = {
  currentNotificationTab: ENotificationTab;
};

export const NotificationEmptyState = observer(function NotificationEmptyState({
  currentNotificationTab,
}: TNotificationEmptyStateProps) {
  // karma-team imports
  const { t } = useTranslation();

  return (
    <>
      <EmptyStateCompact
        assetKey="inbox"
        assetClassName="size-24"
        title={
          currentNotificationTab === ENotificationTab.ALL
            ? t("Updates for your subscribed work items will appear here")
            : t("Mentions for your work items will appear here")
        }
        className="max-w-56"
      />
    </>
  );
});
