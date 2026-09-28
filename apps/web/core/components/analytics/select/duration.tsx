/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

// karma-team package imports
import type { ReactNode } from "react";
import React from "react";
import { Calendar } from "lucide-react";
// karma-team package imports
import { ANALYTICS_DURATION_FILTER_OPTIONS } from "@karma-team/constants";
import { useTranslation } from "@karma-team/i18n";
import { CustomSearchSelect } from "@karma-team/ui";
// types
import type { TDropdownProps } from "@/components/dropdowns/types";

type Props = TDropdownProps & {
  value: string | null;
  onChange: (val: (typeof ANALYTICS_DURATION_FILTER_OPTIONS)[number]["value"]) => void;
  //optional
  button?: ReactNode;
  dropdownArrow?: boolean;
  dropdownArrowClassName?: string;
  onClose?: () => void;
  renderByDefault?: boolean;
  tabIndex?: number;
};

function DurationDropdown({ placeholder = "Duration", onChange, value }: Props) {
  useTranslation();

  const options = ANALYTICS_DURATION_FILTER_OPTIONS.map((option) => ({
    value: option.value,
    query: option.name,
    content: (
      <div className="flex max-w-[300px] items-center gap-2">
        <span className="flex-grow truncate">{option.name}</span>
      </div>
    ),
  }));
  return (
    <CustomSearchSelect
      value={value ? [value] : []}
      onChange={onChange}
      options={options}
      label={
        <div className="flex items-center gap-2 p-1">
          <Calendar className="h-4 w-4" />
          {value ? ANALYTICS_DURATION_FILTER_OPTIONS.find((opt) => opt.value === value)?.name : placeholder}
        </div>
      }
    />
  );
}

export default DurationDropdown;
