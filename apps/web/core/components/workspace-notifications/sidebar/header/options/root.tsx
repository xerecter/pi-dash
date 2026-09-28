/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
import { CheckCheck, RefreshCw } from "lucide-react";
// karma-team imports
import { ENotificationLoader, ENotificationQueryParamType } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { Tooltip } from "@karma-team/propel/tooltip";
import { Spinner } from "@karma-team/ui";
// hooks
import { useWorkspaceNotifications } from "@/hooks/store/notifications";
import { usePlatformOS } from "@/hooks/use-platform-os";
// local imports
import { NotificationFilter } from "../../filters/menu";
import { NotificationHeaderMenuOption } from "./menu-option";
import { IconButton } from "@karma-team/propel/icon-button";

type TNotificationSidebarHeaderOptions = {
  workspaceSlug: string;
};

export const NotificationSidebarHeaderOptions = observer(function NotificationSidebarHeaderOptions(
  props: TNotificationSidebarHeaderOptions
) {
  const { workspaceSlug } = props;
  // hooks
  const { isMobile } = usePlatformOS();
  const { loader, getNotifications, markAllNotificationsAsRead } = useWorkspaceNotifications();
  const { t } = useTranslation();

  const refreshNotifications = async () => {
    if (loader) return;
    try {
      await getNotifications(workspaceSlug, ENotificationLoader.MUTATION_LOADER, ENotificationQueryParamType.CURRENT);
    } catch (error) {
      console.error(error);
    }
  };

  const handleMarkAllNotificationsAsRead = async () => {
    // NOTE: We are using loader to prevent continues request when we are making all the notification to read
    if (loader) return;
    try {
      await markAllNotificationsAsRead(workspaceSlug);
    } catch (error) {
      console.error(error);
    }
  };

  return (
    <div className="relative flex items-center justify-center gap-2 text-body-xs-medium">
      {/* mark all notifications as read*/}
      <Tooltip tooltipContent={t("Mark all as read")} isMobile={isMobile} position="bottom">
        <IconButton
          size="base"
          variant="ghost"
          icon={loader === ENotificationLoader.MARK_ALL_AS_READY ? Spinner : CheckCheck}
          onClick={() => {
            handleMarkAllNotificationsAsRead();
          }}
        />
      </Tooltip>

      {/* refetch current notifications */}
      <Tooltip tooltipContent={t("Refresh")} isMobile={isMobile} position="bottom">
        <IconButton
          size="base"
          variant="ghost"
          icon={RefreshCw}
          className={loader === ENotificationLoader.MUTATION_LOADER ? "animate-spin" : ""}
          onClick={refreshNotifications}
        />
      </Tooltip>

      {/* notification filters */}
      <NotificationFilter />

      {/* notification menu options */}
      <NotificationHeaderMenuOption />
    </div>
  );
});
