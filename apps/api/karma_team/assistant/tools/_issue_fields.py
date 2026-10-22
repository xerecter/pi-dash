# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

"""Validation and through-model writes shared by the issue tools."""

from uuid import UUID

from django.core.exceptions import ValidationError
from pydantic_ai import ModelRetry

from karma_team.assistant.tools import _results
from karma_team.db.models import Issue, IssueAssignee, IssueLabel, Label, Project, ProjectMember, User


def uuid_list(values: list[str]) -> list[UUID]:
    if len(values) > 100:
        raise ModelRetry("Provide at most 100 IDs per operation.")
    try:
        return list(dict.fromkeys(UUID(str(value)) for value in values))
    except (ValueError, TypeError, AttributeError) as exc:
        raise ModelRetry("Use UUIDs returned by the lookup tools.") from exc


def branch_name(value: str) -> str:
    if "\n" in value or "\r" in value:
        raise ModelRetry("A branch name cannot contain line breaks.")
    try:
        return Issue._meta.get_field("git_work_branch").clean(value, None)
    except ValidationError as exc:
        raise ModelRetry("Invalid git_work_branch: " + "; ".join(exc.messages)) from exc


def assignees_for(project: Project, values: list[str]) -> list[UUID]:
    ids = uuid_list(values)
    valid = set(
        ProjectMember.objects.filter(
            project_id=project.id,
            workspace_id=project.workspace_id,
            member_id__in=ids,
            is_active=True,
            role__gte=15,
            member__is_active=True,
        ).values_list("member_id", flat=True)
    )
    if set(ids) != valid:
        raise ModelRetry("Assignees must be active Members or Admins of this project. Use list_project_members.")
    return ids


def labels_for(project: Project, values: list[str]) -> list[UUID]:
    ids = uuid_list(values)
    valid = set(
        Label.objects.filter(
            project_id=project.id,
            workspace_id=project.workspace_id,
            id__in=ids,
        ).values_list("id", flat=True)
    )
    if set(ids) != valid:
        raise ModelRetry("Labels must belong to this project. Use list_labels.")
    return ids


def set_links(issue: Issue, user: User, field: str, ids: list[UUID], operation: str = "set") -> None:
    """The caller holds the issue row lock; preserve unchanged link records."""
    model, key = (IssueAssignee, "assignee_id") if field == "assignees" else (IssueLabel, "label_id")
    links = model.objects.filter(issue=issue)
    if operation == "remove":
        links.filter(**{f"{key}__in": ids}).delete()
        return
    if operation == "set":
        links.exclude(**{f"{key}__in": ids}).delete()
    existing = set(links.values_list(key, flat=True))
    model.objects.bulk_create(
        [
            model(
                issue=issue,
                project_id=issue.project_id,
                workspace_id=issue.workspace_id,
                created_by=user,
                updated_by=user,
                **{key: value},
            )
            for value in ids
            if value not in existing
        ]
    )


def metadata(issue: Issue) -> dict:
    assignees = [
        {
            "id": str(link.assignee_id),
            "display_name": _results.wrap_untrusted(link.assignee.display_name or link.assignee.email),
        }
        for link in IssueAssignee.objects.filter(issue=issue, assignee__is_active=True).select_related("assignee")
    ]
    labels = [
        {"id": str(link.label_id), "name": _results.wrap_untrusted(link.label.name), "color": link.label.color}
        for link in IssueLabel.objects.filter(issue=issue, label__deleted_at__isnull=True).select_related("label")
    ]
    return {
        "git_work_branch": issue.git_work_branch,
        "assignee_ids": [item["id"] for item in assignees],
        "assignees": assignees,
        "label_ids": [item["id"] for item in labels],
        "labels": labels,
    }
