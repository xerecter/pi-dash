# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Workspace-membership and role helpers for the runner app.

Extracted so runner views, pod views, validation, and orchestration can share a
single source of truth. See ``.ai_design/issue_runner/design.md`` §5.1.

Role values come from ``karma_team.db.models.workspace.ROLE_CHOICES``:
``Admin=20``, ``Member=15``, ``Guest=5``.
"""

from __future__ import annotations

from django.db.models import Exists, OuterRef, Q

from karma_team.runner.models import Visibility

# Workspace membership/role helpers now live in the neutral ``karma_team.core``
# package so the assistant app can share them without importing ``runner``.
# Re-exported here for backwards compatibility with existing runner callers.
from karma_team.core.permissions import (  # noqa: F401
    ROLE_ADMIN,
    ROLE_GUEST,
    ROLE_MEMBER,
    is_at_least_member,
    is_workspace_admin,
    is_workspace_member,
    workspace_role,
)


def _authenticated_user_id(user):
    if user is None or not getattr(user, "is_authenticated", False):
        return None
    return getattr(user, "id", None)


def runner_visible_to_user_q(user, *, prefix: str = "") -> Q:
    """ORM predicate for runners visible to ``user``.

    Visibility currently has one public value: ``PRIVATE=0``. Private means the
    row is visible only to its owner, even inside a shared workspace.
    """
    user_id = _authenticated_user_id(user)
    if user_id is None:
        return Q(**{f"{prefix}pk__isnull": True})
    return Q(
        **{
            f"{prefix}owner_id": user_id,
            f"{prefix}visibility": Visibility.PRIVATE,
        }
    )


def scheduler_run_visible_to_user_q(user, *, prefix: str = "") -> Q:
    """ORM predicate for a scheduler run visible to a project member.

    Project membership is deliberately *not* encoded here; scheduler endpoints
    apply their normal project permission before using this predicate.  This is
    the second, private-runner gate shared by history, calendar occurrences and
    binding summary counts:

    - runner-less (queued / Cloud Agent) runs are project-visible;
    - a run on a private runner is visible to its creator or runner owner.

    ``prefix`` targets an ``AgentRun`` relation, for example ``"last_run__"``.
    """
    user_id = _authenticated_user_id(user)
    if user_id is None:
        return Q(**{f"{prefix}pk__isnull": True})
    return (
        Q(**{f"{prefix}runner__isnull": True})
        | Q(**{f"{prefix}runner__owner_id": user_id})
        | Q(**{f"{prefix}created_by_id": user_id})
    )


def can_view_scheduler_run(user, run) -> bool:
    """Object-level equivalent of :func:`scheduler_run_visible_to_user_q`.

    Callers must still enforce active project membership. Keeping this small
    object check beside the queryset predicate prevents binding serializers
    from drifting away from the history/calendar policy.
    """
    user_id = _authenticated_user_id(user)
    if user_id is None or run is None:
        return False
    if run.runner_id is None or run.created_by_id == user_id:
        return True
    return run.runner.owner_id == user_id


def can_view_dev_machine(user, dev_machine) -> bool:
    user_id = _authenticated_user_id(user)
    if user_id is None:
        return False
    if dev_machine.visibility == Visibility.PRIVATE:
        return dev_machine.owner_id == user_id
    return False


def can_use_dev_machine(user, dev_machine) -> bool:
    return can_view_dev_machine(user, dev_machine)


def can_view_runner(user, runner) -> bool:
    user_id = _authenticated_user_id(user)
    if user_id is None:
        return False
    if runner.visibility == Visibility.PRIVATE:
        return runner.owner_id == user_id
    return False


def can_use_runner(user, runner) -> bool:
    return can_view_runner(user, runner)


def filter_runs_usable_by_runner(qs, runner):
    """Limit an AgentRun queryset to work this runner may consume.

    Assigned local issue work belongs to the current assignees' runners.
    For unassigned issues and other work, preserve creator/owner access.

    The assignee clause traverses the ``IssueAssignee`` through-model
    explicitly rather than the ``Issue.assignees`` M2M. ``IssueAssignee`` is
    a ``SoftDeleteModel`` and the un-assign path (issue serializer) only
    stamps ``deleted_at``; a plain M2M join ignores that column, so going
    through the M2M would keep granting execution rights to every user ever
    assigned to the issue. ``deleted_at__isnull=True`` sits in the same
    ``Q``/``filter()`` call as the owner match so both conditions bind to the
    same joined row.
    """
    if runner.visibility == Visibility.PRIVATE:
        from karma_team.core.agent_execution import AgentExecutorKind
        from karma_team.db.models import ProjectMember, WorkspaceMember
        from karma_team.db.models.issue import Issue, IssueAssignee
        from karma_team.db.models.scheduler import SchedulerBinding

        if not (
            WorkspaceMember.objects.filter(
                workspace_id=runner.workspace_id, member_id=runner.owner_id, is_active=True
            ).exists()
            and ProjectMember.objects.filter(
                workspace_id=runner.workspace_id,
                project_id=runner.pod.project_id,
                member_id=runner.owner_id,
                is_active=True,
            ).exists()
        ):
            # Keep non-issue work unaffected; a stale runner must not gain
            # access to a project's work items after its owner leaves.
            qs = qs.filter(work_item__isnull=True)

        visible_issue = Issue.objects.filter(pk=OuterRef("work_item_id")).filter(
            Q(created_by_id=runner.owner_id)
            | Q(
                issue_assignee__assignee_id=runner.owner_id,
                issue_assignee__deleted_at__isnull=True,
            )
        )
        visible_scheduler = SchedulerBinding.objects.filter(
            pk=OuterRef("scheduler_binding_id"),
            actor_id=runner.owner_id,
        )
        assignees = IssueAssignee.objects.filter(issue_id=OuterRef("work_item_id"))
        return (
            qs.annotate(
                _runner_visible_issue=Exists(visible_issue),
                _runner_visible_scheduler=Exists(visible_scheduler),
                _issue_has_assignees=Exists(assignees),
                _issue_assigned_to_runner=Exists(assignees.filter(assignee_id=runner.owner_id)),
            )
            .filter(
                Q(created_by_id=runner.owner_id)
                | Q(owner_id=runner.owner_id)
                | Q(_runner_visible_issue=True)
                | Q(_runner_visible_scheduler=True)
            )
            .filter(
                ~Q(executor_kind=AgentExecutorKind.LOCAL_RUNNER)
                | Q(_issue_has_assignees=False)
                | Q(_issue_assigned_to_runner=True)
            )
        )
    return qs.none()


def can_manage_runner(user, runner) -> bool:
    """True if ``user`` may mutate the runner.

    Shared by every runner view (revoke/revive/delete/patch) and both
    delete surfaces (web session auth + the X-Api-Key v1 endpoint), so
    the rule lives in one place. Private runners are owner-managed only.
    """
    if not can_view_runner(user, runner):
        return False
    if runner.visibility == Visibility.PRIVATE:
        return runner.owner_id == user.id
    return runner.owner_id == user.id or is_workspace_admin(user, runner.workspace_id)
