/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { useFormContext } from "react-hook-form";
// karma-team imports
import { ETabIndices } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { Button } from "@karma-team/propel/button";
import type { IProject } from "@karma-team/types";
// ui
// helpers
import { getTabIndex } from "@karma-team/utils";

type Props = {
  handleClose: () => void;
  isMobile?: boolean;
};

function ProjectCreateButtons(props: Props) {
  const { t } = useTranslation();
  const { handleClose, isMobile = false } = props;
  const {
    formState: { isSubmitting },
  } = useFormContext<IProject>();

  const { getIndex } = getTabIndex(ETabIndices.PROJECT_CREATE, isMobile);

  return (
    <div className="flex justify-end gap-2 border-t border-subtle py-4">
      <Button variant="secondary" size="lg" onClick={handleClose} tabIndex={getIndex("cancel")}>
        {t("Cancel")}
      </Button>
      <Button variant="primary" size="lg" type="submit" loading={isSubmitting} tabIndex={getIndex("submit")}>
        {isSubmitting ? t("Creating") : t("Create project")}
      </Button>
    </div>
  );
}

export default ProjectCreateButtons;
