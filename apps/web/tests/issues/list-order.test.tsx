import { act, render, screen } from "@testing-library/react";
import { observable, runInAction } from "mobx";
import { describe, expect, it, vi } from "vitest";
import type { IIssueFilters, TGroupedIssues, TIssue, TIssueMap } from "@karma-team/types";
import { sortIssueIdsByParentAndSequence, withProjectListOrder } from "@/helpers/issue-list-order";
import { BaseListRoot } from "@/components/issues/issue-layouts/list/base-list-root";
import { ProjectIssuesFilter } from "@/store/issue/project/filter.store";
import type { IIssueRootStore } from "@/store/issue/root.store";

vi.mock("@/services/project", () => ({ ProjectService: vi.fn() }));

const fixture = vi.hoisted(() => ({
  groups: {} as TGroupedIssues,
  issues: {} as TIssueMap,
  order: "parent_sequence",
  fetch: vi.fn(),
}));
vi.mock("next/navigation", () => ({ useParams: () => ({ workspaceSlug: "test", projectId: "project" }) }));
vi.mock("@/hooks/store/use-issues", () => ({
  useIssues: () => ({
    issueMap: fixture.issues,
    issues: { groupedIssueIds: fixture.groups, viewFlags: {} },
    issuesFilter: { issueFilters: { displayFilters: { layout: "list", order_by: fixture.order } } },
  }),
}));
vi.mock("@/hooks/store/user", () => ({ useUserPermissions: () => ({ allowPermissions: () => true }) }));
vi.mock("@/hooks/use-group-dragndrop", () => ({ useGroupIssuesDragNDrop: () => vi.fn() }));
vi.mock("@/hooks/use-issue-layout-store", () => ({ useIssueStoreType: () => "PROJECT" }));
vi.mock("@/hooks/use-issues-actions", () => ({ useIssuesActions: () => ({ fetchIssues: fixture.fetch }) }));
vi.mock("@/components/issues/issue-layouts/issue-layout-HOC", () => ({
  IssueLayoutHOC: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("@/components/issues/issue-layouts/list/default", () => ({
  List: ({ groupedIssueIds }: { groupedIssueIds: TGroupedIssues }) => (
    <output>{JSON.stringify(groupedIssueIds)}</output>
  ),
}));

function issue(sequence: number, parent: string | null = null): TIssue {
  return { id: String(sequence), sequence_id: sequence, parent_id: parent } as TIssue;
}

describe("project list order", () => {
  it("sorts parents and children numerically without changing the input", () => {
    const items = [issue(11, "1"), issue(20), issue(3, "2"), issue(10), issue(2), issue(1, "10")];
    const map = Object.fromEntries(items.map((item) => [item.id, item]));
    const ids = items.map((item) => item.id);
    expect(sortIssueIdsByParentAndSequence(ids, map)).toEqual(["2", "10", "20", "1", "3", "11"]);
    expect(ids).toEqual(["11", "20", "3", "10", "2", "1"]);
    expect(sortIssueIdsByParentAndSequence(["11", "3", "1"], map)).toEqual(["1", "3", "11"]);
  });

  it("uses the same order in requests and display without mutating saved preferences", () => {
    const saved: IIssueFilters = {
      richFilters: {},
      displayProperties: undefined,
      kanbanFilters: undefined,
      displayFilters: { layout: "list", order_by: "-created_at" },
    };
    expect(withProjectListOrder(saved).displayFilters?.order_by).toBe("parent_sequence");
    expect(saved.displayFilters?.order_by).toBe("-created_at");
    const board: IIssueFilters = { ...saved, displayFilters: { layout: "kanban", order_by: "sort_order" } };
    expect(withProjectListOrder(board)).toBe(board);
  });

  it("reorders every rendered group when an item is reparented, detached, or appended", () => {
    fixture.order = "parent_sequence";
    fixture.groups = observable({ first: ["10", "2", "1"], second: ["11", "3"] });
    fixture.issues = observable({
      "1": issue(1, "10"),
      "2": issue(2),
      "3": issue(3),
      "10": issue(10),
      "11": issue(11, "3"),
    });
    render(<BaseListRoot QuickActions={() => null} />);
    const rendered = () => JSON.parse(screen.getByRole("status").textContent ?? "{}");
    expect(rendered()).toEqual({ first: ["2", "10", "1"], second: ["3", "11"] });
    act(() =>
      runInAction(() => {
        fixture.issues["2"].parent_id = "10";
      })
    );
    expect(rendered().first).toEqual(["10", "1", "2"]);
    act(() =>
      runInAction(() => {
        fixture.issues["1"].parent_id = null;
      })
    );
    expect(rendered().first).toEqual(["1", "10", "2"]);
    act(() =>
      runInAction(() => {
        fixture.issues["4"] = issue(4);
        fixture.groups.first.push("4");
      })
    );
    expect(rendered().first).toEqual(["1", "4", "10", "2"]);
  });

  it("preserves other views' existing ordering", () => {
    fixture.order = "-created_at";
    fixture.groups = { first: ["10", "2", "1"] };
    fixture.issues = { "1": issue(1), "2": issue(2), "10": issue(10) };
    render(<BaseListRoot QuickActions={() => null} />);
    expect(JSON.parse(screen.getByRole("status").textContent ?? "{}")).toEqual(fixture.groups);
  });

  it("sends the project list order on initial and subsequent page requests", () => {
    const filters = new ProjectIssuesFilter({ projectId: "project" } as IIssueRootStore);
    runInAction(() => {
      filters.filters.project = {
        richFilters: {},
        displayProperties: undefined,
        kanbanFilters: undefined,
        displayFilters: { layout: "list", order_by: "-created_at" },
      };
    });
    for (const cursor of [undefined, "2:1:0"]) {
      const params = filters.getFilterParams(
        { canGroup: true, perPageCount: 2 },
        "project",
        cursor,
        undefined,
        undefined
      );
      expect(params.order_by).toBe("parent_sequence");
      expect(params.layout).toBe("list");
    }
    runInAction(() => {
      filters.filters.project.displayFilters!.layout = "kanban";
    });
    expect(filters.getAppliedFilters("project")?.order_by).toBe("-created_at");
  });
});
