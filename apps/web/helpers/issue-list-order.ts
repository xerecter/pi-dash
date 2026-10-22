import { orderBy } from "lodash-es";
import type { IIssueFilters, TIssueMap } from "@karma-team/types";

export function withProjectListOrder(filters: IIssueFilters): IIssueFilters {
  if (filters.displayFilters?.layout !== "list") return filters;
  return { ...filters, displayFilters: { ...filters.displayFilters, order_by: "parent_sequence" } };
}

/** Read observable fields during rendering so reparenting immediately moves a row. */
export function sortIssueIdsByParentAndSequence(issueIds: string[], issues: TIssueMap): string[] {
  return orderBy(issueIds, [
    (id) => Number(Boolean(issues[id]?.parent_id)),
    (id) => issues[id]?.sequence_id ?? Number.MAX_SAFE_INTEGER,
    (id) => id,
  ]);
}
