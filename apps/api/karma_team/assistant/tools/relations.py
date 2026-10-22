# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

"""Issue relations, expressed from the requested issue's perspective."""

from typing import Literal

from crum import impersonate
from django.db import transaction
from django.db.models import Q
from pydantic_ai import ModelRetry, RunContext

from karma_team.assistant.runtime.agent import assistant
from karma_team.assistant.runtime.deps import AssistantDeps
from karma_team.assistant.tools import _issue_fields, _results, _scoping
from karma_team.assistant.tools.issues import _identifier
from karma_team.db.models import Issue, IssueRelation
from karma_team.orchestration.signals import schedule_unblocked_issue_wake
from karma_team.utils.issue_relation_mapper import get_actual_relation, get_inverse_relation

RelationType = Literal[
    "blocked_by",
    "blocking",
    "relates_to",
    "duplicate",
    "start_before",
    "start_after",
    "finish_before",
    "finish_after",
    "implemented_by",
    "implements",
]
_REVERSED = {"blocking", "start_after", "finish_after", "implements"}
_VALID = _REVERSED | {"blocked_by", "relates_to", "duplicate", "start_before", "finish_before", "implemented_by"}


def _pair(issue_id, other_id) -> Q:
    return Q(issue_id=issue_id, related_issue_id=other_id) | Q(issue_id=other_id, related_issue_id=issue_id)


def _result(relation: IssueRelation, issue: Issue) -> dict:
    forward = relation.issue_id == issue.id
    other = relation.related_issue if forward else relation.issue
    return {
        "id": str(relation.id),
        "issue_id": str(issue.id),
        "relation_type": relation.relation_type if forward else get_inverse_relation(relation.relation_type),
        "related_issue_id": str(other.id),
        "related_issue_identifier": _identifier(other),
        "related_issue_name": _results.wrap_untrusted(other.name),
    }


@assistant.tool
def list_issue_relations(ctx: RunContext[AssistantDeps], issue_id: str) -> list[dict]:
    """Read all visible relations of an issue, including inverse relationships.
    Relation IDs can be passed to update_issue_relation or delete_issue_relation."""
    issue = _scoping.get_issue(ctx.deps, issue_id)
    accessible = _scoping.scoped_issues(ctx.deps).values("id")
    relations = (
        IssueRelation.objects.filter(
            Q(issue=issue) | Q(related_issue=issue),
            workspace_id=ctx.deps.workspace_id,
            issue_id__in=accessible,
            related_issue_id__in=accessible,
        )
        .select_related("issue__project", "related_issue__project")
        .order_by("created_at", "id")
    )
    return [_result(relation, issue) for relation in relations]


def _write_pair(deps: AssistantDeps, issue_id: str, other_id: str) -> tuple[Issue, Issue]:
    issue = _scoping.get_issue(deps, issue_id)
    other = _scoping.get_issue(deps, other_id)
    if issue.id == other.id:
        raise ModelRetry("An issue cannot be related to itself.")
    for item in (issue, other):
        _scoping.require_project_write(deps, str(item.project_id))
    return issue, other


def _lock_pair(issue: Issue, other: Issue) -> None:
    # Lock in a stable order so edits from either end serialize together.
    locked = list(Issue.objects.filter(pk__in=[issue.pk, other.pk]).order_by("pk").select_for_update())
    if len(locked) != 2:
        raise _scoping.ToolNotFound("One of the related issues is no longer available.")


def _endpoints(issue: Issue, other: Issue, relation_type: str) -> tuple[Issue, Issue, str]:
    if relation_type not in _VALID:
        raise ModelRetry(f"Relation type must be one of {sorted(_VALID)}.")
    source, target = (other, issue) if relation_type in _REVERSED else (issue, other)
    return source, target, get_actual_relation(relation_type)


def _get_relation(ctx, issue_id: str, relation_id: str) -> tuple[Issue, Issue, IssueRelation]:
    issue = _scoping.get_issue(ctx.deps, issue_id)
    relation_uuid = _issue_fields.uuid_list([relation_id])[0]
    relation = (
        IssueRelation.objects.filter(
            Q(issue=issue) | Q(related_issue=issue),
            id=relation_uuid,
            workspace_id=ctx.deps.workspace_id,
        )
        .select_related("issue__project", "related_issue__project")
        .first()
    )
    if relation is None:
        raise _scoping.ToolNotFound("Relation not found on this issue.")
    other_id = relation.related_issue_id if relation.issue_id == issue.id else relation.issue_id
    issue, other = _write_pair(ctx.deps, issue_id, str(other_id))
    return issue, other, relation


def _record(deps, issue: Issue, action: str) -> None:
    _results.record_write(deps, f"{action} relation on {_identifier(issue)}", links=[_results.issue_link(deps, issue)])


@assistant.tool
def create_issue_relation(
    ctx: RunContext[AssistantDeps],
    issue_id: str,
    related_issue_id: str,
    relation_type: RelationType,
) -> dict:
    """Link two accessible issues. Requires write access to both projects.
    'blocked_by' means issue_id waits for related_issue_id; 'blocking' is the inverse.
    An existing identical relation is a no-op; use update to change its type."""
    issue, other = _write_pair(ctx.deps, issue_id, related_issue_id)
    source, target, kind = _endpoints(issue, other, relation_type)
    user = _scoping.user_for(ctx.deps)
    with impersonate(user), transaction.atomic():
        _lock_pair(issue, other)
        relation = IssueRelation.objects.filter(_pair(issue.id, other.id)).first()
        if relation is not None:
            same = relation.relation_type == kind and (
                kind in {"relates_to", "duplicate"} or relation.issue_id == source.id
            )
            if not same:
                raise ModelRetry("These issues already have a relation. Use update_issue_relation to change it.")
            return {"created": False, **_result(relation, issue)}
        relation = IssueRelation.objects.create(
            issue=source,
            related_issue=target,
            relation_type=kind,
            project=source.project,
            workspace_id=ctx.deps.workspace_id,
        )
    _record(ctx.deps, issue, "Created")
    return {"created": True, **_result(relation, issue)}


@assistant.tool
def update_issue_relation(
    ctx: RunContext[AssistantDeps],
    issue_id: str,
    relation_id: str,
    relation_type: RelationType,
) -> dict:
    """Change a relation's type/direction from issue_id's perspective.
    Removing a blocking dependency wakes eligible work only after commit.
    To link a different issue, delete this relation and create a new one."""
    issue, other, relation = _get_relation(ctx, issue_id, relation_id)
    source, target, kind = _endpoints(issue, other, relation_type)
    user = _scoping.user_for(ctx.deps)
    with impersonate(user), transaction.atomic():
        _lock_pair(issue, other)
        relation = IssueRelation.objects.select_for_update().get(pk=relation.pk)
        if IssueRelation.objects.filter(_pair(issue.id, other.id)).exclude(pk=relation.pk).exists():
            raise ModelRetry("Multiple relations exist between these issues. Remove the conflicting relation first.")
        blocker = relation.related_issue if relation.relation_type == "blocked_by" else None
        blocked_id = relation.issue_id
        relation.issue, relation.related_issue = source, target
        relation.relation_type = kind
        relation.project = source.project
        relation.save(update_fields=["issue", "related_issue", "relation_type", "project", "updated_at", "updated_by"])
        if blocker is not None:
            schedule_unblocked_issue_wake(blocker, actor=user, blocked_issue_ids=[blocked_id])
    _record(ctx.deps, issue, "Updated")
    return {"updated": True, **_result(relation, issue)}


@assistant.tool
def delete_issue_relation(ctx: RunContext[AssistantDeps], issue_id: str, relation_id: str) -> dict:
    """Remove one relation, without deleting either issue. Requires write access to both.
    Removing the last unresolved dependency can resume an eligible coding run."""
    _scoping.require_interactive_delete(ctx.deps)
    issue, other, relation = _get_relation(ctx, issue_id, relation_id)
    user = _scoping.user_for(ctx.deps)
    with impersonate(user), transaction.atomic():
        _lock_pair(issue, other)
        relation = IssueRelation.objects.select_for_update().get(pk=relation.pk)
        result = _result(relation, issue)
        relation.delete()
        if relation.relation_type == "blocked_by":
            schedule_unblocked_issue_wake(relation.related_issue, actor=user, blocked_issue_ids=[relation.issue_id])
    _record(ctx.deps, issue, "Deleted")
    return {"deleted": True, **result}
