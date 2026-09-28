/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

import React, { useState } from "react";
import { observer } from "mobx-react";
// karma-team constants
import { ISSUE_PRIORITIES } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
// ui
import { PriorityIcon } from "@karma-team/propel/icons";

// components
import { FilterHeader, FilterOption } from "@/components/issues/issue-layouts/filters";
type Props = {
  appliedFilters: string[] | null;
  handleUpdate: (val: string) => void;
  searchQuery: string;
};

export const FilterPriority = observer(function FilterPriority(props: Props) {
  const { appliedFilters, handleUpdate, searchQuery } = props;
  // hooks
  const { t } = useTranslation();
  const [previewEnabled, setPreviewEnabled] = useState(true);

  const appliedFiltersCount = appliedFilters?.length ?? 0;

  const filteredOptions = ISSUE_PRIORITIES.filter((p) => p.key.includes(searchQuery.toLowerCase()));
  return (
    <>
      <FilterHeader
        title={`Priority ${appliedFiltersCount > 0 ? ` (${appliedFiltersCount})` : ""}`}
        isPreviewEnabled={previewEnabled}
        handleIsPreviewEnabled={() => setPreviewEnabled(!previewEnabled)}
      />
      {previewEnabled && (
        <div>
          {filteredOptions.length > 0 ? (
            filteredOptions.map((priority) => (
              <FilterOption
                key={priority.key}
                isChecked={appliedFilters?.includes(priority.key) ? true : false}
                onClick={() => handleUpdate(priority.key)}
                icon={<PriorityIcon priority={priority.key} className="h-3.5 w-3.5" />}
                title={priority.title}
              />
            ))
          ) : (
            <p className="text-11 text-placeholder italic">{t("No matches found")}</p>
          )}
        </div>
      )}
    </>
  );
});
