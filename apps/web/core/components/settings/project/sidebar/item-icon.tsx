/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import type { LucideIcon } from "lucide-react";
import { CalendarClock, GitBranch, Users, Zap } from "lucide-react";
// karma-team imports
import type { ISvgIcons } from "@karma-team/propel/icons";
import {
  CycleIcon,
  EstimatePropertyIcon,
  IntakeIcon,
  LabelPropertyIcon,
  ModuleIcon,
  PageIcon,
  StatePropertyIcon,
  ViewsIcon,
} from "@karma-team/propel/icons";
import type { TProjectSettingsTabs } from "@karma-team/types";
// components
import { SettingIcon } from "@/components/icons/attachment";

export const PROJECT_SETTINGS_ICONS: Record<TProjectSettingsTabs, LucideIcon | React.FC<ISvgIcons>> = {
  general: SettingIcon,
  members: Users,
  features_cycles: CycleIcon,
  features_modules: ModuleIcon,
  features_views: ViewsIcon,
  features_pages: PageIcon,
  features_intake: IntakeIcon,
  states: StatePropertyIcon,
  labels: LabelPropertyIcon,
  estimates: EstimatePropertyIcon,
  automations: Zap,
  github: GitBranch,
  schedulers: CalendarClock,
};
