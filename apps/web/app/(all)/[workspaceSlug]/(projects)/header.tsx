/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
import { Shapes } from "lucide-react";
// karma-team imports
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";
import { HomeIcon } from "@karma-team/propel/icons";
import { Breadcrumbs, Header } from "@karma-team/ui";
// components
import { BreadcrumbLink } from "@/components/common/breadcrumb-link";
// hooks
import { useHome } from "@/hooks/store/use-home";

export const WorkspaceDashboardHeader = observer(function WorkspaceDashboardHeader() {
  // karma-team hooks
  const { t } = useTranslation();
  // hooks
  const { toggleWidgetSettings } = useHome();

  return (
    <>
      <Header>
        <Header.LeftItem>
          <div className="flex items-center gap-2">
            <Breadcrumbs>
              <Breadcrumbs.Item
                component={<BreadcrumbLink label={t("Home")} icon={<HomeIcon className="h-4 w-4 text-tertiary" />} />}
              />
            </Breadcrumbs>
          </div>
        </Header.LeftItem>
        <Header.RightItem>
          <Button
            variant="secondary"
            size="lg"
            onClick={() => toggleWidgetSettings(true)}
            className="my-auto mb-0"
            prependIcon={<Shapes />}
          >
            <div className="hidden sm:hidden md:block">{t("Manage widgets")}</div>
          </Button>
        </Header.RightItem>
      </Header>
    </>
  );
});
