/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { TNetworkChoiceIconKey } from "@karma-team/constants";
// karma-team imports
import { GlobeIcon, LockIcon } from "@karma-team/propel/icons";
import { cn } from "@karma-team/utils";

type Props = {
  iconKey: TNetworkChoiceIconKey;
  className?: string;
};

export function ProjectNetworkIcon(props: Props) {
  const { iconKey, className } = props;
  // Get the icon key
  const getProjectNetworkIcon = () => {
    switch (iconKey) {
      case "Lock":
        return LockIcon;
      case "Globe2":
        return GlobeIcon;
      default:
        return null;
    }
  };

  // Get the icon
  const Icon = getProjectNetworkIcon();
  if (!Icon) return null;

  return <Icon className={cn("h-3 w-3", className)} />;
}
