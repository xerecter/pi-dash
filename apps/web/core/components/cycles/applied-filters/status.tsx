/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import { observer } from "mobx-react";
// karma-team imports
import { CYCLE_STATUS } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { CloseIcon } from "@karma-team/propel/icons";
import { cn } from "@karma-team/utils";

type Props = {
  handleRemove: (val: string) => void;
  values: string[];
  editable: boolean | undefined;
};

export const AppliedStatusFilters = observer(function AppliedStatusFilters(props: Props) {
  const { handleRemove, values, editable } = props;
  const { t } = useTranslation();

  return (
    <>
      {values.map((status) => {
        const statusDetails = CYCLE_STATUS.find((s) => s.value === status);
        return (
          <div
            key={status}
            className={cn(
              "flex items-center gap-1 rounded-sm px-1.5 py-1 text-11",
              statusDetails?.bgColor,
              statusDetails?.textColor
            )}
          >
            {statusDetails && t(statusDetails?.i18n_title)}
            {editable && (
              <button
                type="button"
                className="grid place-items-center text-tertiary hover:text-secondary"
                onClick={() => handleRemove(status)}
              >
                <CloseIcon height={10} width={10} strokeWidth={2} />
              </button>
            )}
          </div>
        );
      })}
    </>
  );
});
