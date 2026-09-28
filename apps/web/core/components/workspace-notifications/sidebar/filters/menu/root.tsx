/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
import { ListFilter } from "lucide-react";
// karma-team imports
import type { ENotificationFilterType } from "@karma-team/constants";
import { FILTER_TYPE_OPTIONS } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { Tooltip } from "@karma-team/propel/tooltip";
import { PopoverMenu } from "@karma-team/ui";
// hooks
import { usePlatformOS } from "@/hooks/use-platform-os";
// local imports
import { NotificationFilterOptionItem } from "./menu-option-item";
import { IconButton } from "@karma-team/propel/icon-button";

export const NotificationFilter = observer(function NotificationFilter() {
  // hooks
  const { isMobile } = usePlatformOS();
  const { t } = useTranslation();

  const translatedFilterTypeOptions = FILTER_TYPE_OPTIONS.map((filter) => ({
    ...filter,
    label: t(filter.i18n_label),
  }));

  return (
    <PopoverMenu
      data={translatedFilterTypeOptions}
      button={
        <Tooltip tooltipContent={t("Inbox Filters")} isMobile={isMobile} position="bottom">
          <IconButton size="base" variant="ghost" icon={ListFilter} />
        </Tooltip>
      }
      keyExtractor={(item: { label: string; value: ENotificationFilterType }) => item.value}
      render={(item) => <NotificationFilterOptionItem {...item} />}
    />
  );
});
