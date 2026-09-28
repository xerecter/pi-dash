/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { LucideIcon } from "lucide-react";
import { ArrowUpToLine, Blocks, Building, CreditCard, Users, Webhook } from "lucide-react";
// karma-team imports
import type { ISvgIcons } from "@karma-team/propel/icons";
import type { TWorkspaceSettingsTabs } from "@karma-team/types";

export const WORKSPACE_SETTINGS_ICONS: Record<TWorkspaceSettingsTabs, LucideIcon | React.FC<ISvgIcons>> = {
  general: Building,
  members: Users,
  export: ArrowUpToLine,
  "billing-and-plans": CreditCard,
  webhooks: Webhook,
  integrations: Blocks,
};
