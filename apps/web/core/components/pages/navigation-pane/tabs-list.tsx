/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { Tabs } from "@karma-team/propel/tabs";
// karma-team web components
import { ORDERED_PAGE_NAVIGATION_TABS_LIST } from "@/karma-team-web/components/pages/navigation-pane";

export function PageNavigationPaneTabsList() {
  // translation
  const { t } = useTranslation();

  return (
    <div className="mx-3.5">
      <Tabs.List>
        {ORDERED_PAGE_NAVIGATION_TABS_LIST.map((tab) => (
          <Tabs.Trigger key={tab.key} value={tab.key}>
            {t(tab.i18n_label)}
          </Tabs.Trigger>
        ))}
        <Tabs.Indicator />
      </Tabs.List>
    </div>
  );
}
