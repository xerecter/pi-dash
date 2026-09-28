/**
 * Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 * See the LICENSE file for details.
 */

// karma-team imports
import type { IWorkItemFilterInstance } from "@karma-team/shared-state";
import type { EIssuesStoreType } from "@karma-team/types";
// local imports
import { useWorkItemFilters } from "./use-work-item-filters";

export const useWorkItemFilterInstance = (
  entityType: EIssuesStoreType,
  entityId: string | undefined
): IWorkItemFilterInstance | undefined => {
  const { getFilter } = useWorkItemFilters();
  return entityId ? getFilter(entityType, entityId) : undefined;
};
