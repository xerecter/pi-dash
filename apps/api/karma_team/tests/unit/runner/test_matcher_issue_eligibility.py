# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Matcher-level eligibility for *issue-backed* runs in a shared pod.

Assigned local work is dispatched to a current assignee's PRIVATE runner.
Unassigned work retains creator/owner eligibility. Assignment lets a second
member process work they did not create without the creator stealing it.

Scenario throughout: one project, one default pod, two members (Alice
and Luke) who each registered a private runner into that pod.
"""

from __future__ import annotations

from unittest.mock import patch
from uuid import uuid4

import pytest
from crum import impersonate
from django.db import transaction
from django.utils import timezone

from karma_team.db.models import Issue, ProjectMember, State, User, Workspace, WorkspaceMember
from karma_team.db.models.issue import IssueAssignee
from karma_team.orchestration.scheduling import _has_pending_auto_project_binding
from karma_team.runner.models import (
    AgentRun,
    AgentRunStatus,
    DevMachine,
    MachineToken,
    Pod,
    Runner,
    RunnerStatus,
)
from karma_team.runner.services import matcher


@pytest.fixture
def pod(project):
    return Pod.default_for_project(project)


@pytest.fixture
def alice(create_user, workspace, project):
    """Workspace owner from the conftest fixture; the issue creator."""
    ProjectMember.objects.get_or_create(
        workspace=workspace, project=project, member=create_user, defaults={"role": 20}
    )
    return create_user


@pytest.fixture
def luke(db, workspace, project):
    suffix = uuid4().hex[:6]
    user = User.objects.create(
        email=f"luke-{suffix}@example.com",
        username=f"luke_{suffix}",
        first_name="Luke",
    )
    user.set_password("pw")
    user.save()
    WorkspaceMember.objects.create(workspace=workspace, member=user, role=15)
    ProjectMember.objects.create(
        workspace=workspace,
        project=project,
        member=user,
        role=15,
    )
    return user


def _runner(owner, workspace, pod, name):
    return Runner.objects.create(
        owner=owner,
        workspace=workspace,
        pod=pod,
        name=name,
        status=RunnerStatus.ONLINE,
        last_heartbeat_at=timezone.now(),
    )


@pytest.fixture
def alice_runner(alice, workspace, pod):
    return _runner(alice, workspace, pod, "alice-mbp")


@pytest.fixture
def luke_runner(luke, workspace, pod):
    return _runner(luke, workspace, pod, "luke-desktop")


@pytest.fixture
def backlog(project, create_user):
    with impersonate(create_user):
        return State.objects.create(name="Backlog", project=project, group="backlog", default=True, sequence=100)


@pytest.fixture(autouse=True)
def _stub_send_to_runner():
    with patch("karma_team.runner.services.pubsub.send_to_runner") as mock:
        yield mock


@pytest.fixture(autouse=True)
def _on_commit_immediate():
    with patch("django.db.transaction.on_commit", side_effect=lambda fn, **kw: fn()):
        yield


def _issue(workspace, project, backlog, creator, assignees=()):
    """Create an issue in Backlog (non-ticking, so no dispatch auto-fires)."""
    with impersonate(creator):
        issue = Issue.objects.create(
            name="Task",
            workspace=workspace,
            project=project,
            state=backlog,
            created_by=creator,
        )
    for user in assignees:
        IssueAssignee.objects.create(issue=issue, assignee=user, workspace=workspace, project=project)
    return issue


def _queued_run(workspace, pod, issue):
    """Mirror the orchestration dispatch: created_by = issue.created_by,
    owner unset until a runner is assigned."""
    return AgentRun.objects.create(
        workspace=workspace,
        created_by=issue.created_by,
        owner=None,
        pod=pod,
        work_item=issue,
        status=AgentRunStatus.QUEUED,
        prompt="do the thing",
    )


def _visible_to(runner, run):
    """True if the matcher would hand ``run`` to ``runner``."""
    with transaction.atomic():
        picked = matcher.next_for_runner(runner)
    return picked is not None and picked.pk == run.pk


# ---------------------------------------------------------------------------
# Row 1: created by Alice, assigned to Luke -> Luke's runners only
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_assignee_runner_is_eligible_for_issue_it_did_not_create(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)

    assert _visible_to(alice_runner, run) is False, "assigned work belongs to the assignee"
    assert _visible_to(luke_runner, run) is True, "assignee's runner must be eligible"


@pytest.mark.unit
def test_assigned_issue_is_actually_assigned_by_drain_pod(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    """Even a fresher creator runner must not take Luke's assigned work."""
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    alice_runner.last_heartbeat_at = timezone.now()
    alice_runner.save(update_fields=["last_heartbeat_at"])

    assert matcher.drain_pod(pod) == 1
    run.refresh_from_db()
    assert run.runner_id == luke_runner.id
    assert run.status == AgentRunStatus.ASSIGNED
    assert run.owner_id == luke.id, "billing follows the machine, not created_by"


@pytest.mark.unit
def test_assigned_issue_waits_when_only_creator_has_a_runner(
    db, workspace, project, pod, backlog, alice, luke, alice_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    assert matcher.pod_has_runner_for_issue_principal(pod, issue, alice.id) is False
    assert matcher.drain_for_runner(alice_runner) is False
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED


@pytest.mark.unit
def test_removed_project_member_cannot_take_assigned_issue(
    db, workspace, project, pod, backlog, alice, luke, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    ProjectMember.objects.filter(project=project, member=luke).update(is_active=False)

    assert matcher.pod_has_runner_for_issue_principal(pod, issue, alice.id) is False
    assert matcher.drain_for_runner(luke_runner) is False
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED


@pytest.mark.unit
def test_removed_workspace_member_cannot_take_assigned_issue(
    db, workspace, project, pod, backlog, alice, luke, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    WorkspaceMember.objects.filter(workspace=workspace, member=luke).update(is_active=False)

    assert matcher.pod_has_runner_for_issue_principal(pod, issue, alice.id) is False
    assert matcher.drain_for_runner(luke_runner) is False
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED


@pytest.mark.unit
def test_pending_machine_binding_requires_active_project_membership_and_token(
    db, workspace, project, backlog, alice, luke
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    other_workspace = Workspace.objects.create(
        name="Machine home", slug=f"machine-home-{uuid4().hex[:8]}", owner=luke
    )
    home_membership = WorkspaceMember.objects.create(
        workspace=other_workspace, member=luke, role=20
    )
    machine = DevMachine.objects.create(owner=luke, host_label="other-workspace-host")
    token = MachineToken.objects.create(
        user=luke,
        workspace=other_workspace,
        dev_machine=machine,
        host_label="other-workspace-host",
        token_hash="other-workspace-machine-token-hash",
    )
    project_member = ProjectMember.objects.get(project=project, member=luke)
    project.network = 0
    project.save(update_fields=["network"])

    assert _has_pending_auto_project_binding(issue, alice) is True
    project.network = 2
    project.save(update_fields=["network"])
    project_member.is_active = False
    project_member.save(update_fields=["is_active"])
    assert _has_pending_auto_project_binding(issue, alice) is False
    project_member.is_active = True
    project_member.save(update_fields=["is_active"])
    token.revoke()
    assert _has_pending_auto_project_binding(issue, alice) is False
    token.revoked_at = None
    token.save(update_fields=["revoked_at"])
    home_membership.is_active = False
    home_membership.save(update_fields=["is_active"])
    assert _has_pending_auto_project_binding(issue, alice) is False


@pytest.mark.unit
def test_reassignment_moves_execution_to_new_assignees_runner(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    from karma_team.orchestration.service import _pinned_runner_for

    issue = _issue(workspace, project, backlog, creator=alice, assignees=[alice])
    previous = _queued_run(workspace, pod, issue)
    previous.runner = alice_runner
    previous.status = AgentRunStatus.COMPLETED
    previous.save(update_fields=["runner", "status"])
    IssueAssignee.objects.filter(issue=issue).delete()
    IssueAssignee.objects.create(issue=issue, assignee=luke, workspace=workspace, project=project)

    assert _pinned_runner_for(previous, pod) is None
    run = _queued_run(workspace, pod, issue)
    assert _visible_to(alice_runner, run) is False
    assert matcher.drain_for_runner(luke_runner) is True
    run.refresh_from_db()
    assert run.runner_id == luke_runner.id


@pytest.mark.unit
def test_multiple_current_assignees_can_each_serve_the_issue(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[alice, luke])
    run = _queued_run(workspace, pod, issue)
    assert _visible_to(alice_runner, run) is True
    assert _visible_to(luke_runner, run) is True


# ---------------------------------------------------------------------------
# Row 2: created by Alice, unassigned -> only Alice's runner
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_unassigned_issue_is_invisible_to_other_members_runner(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[])
    run = _queued_run(workspace, pod, issue)

    assert _visible_to(alice_runner, run) is True
    assert _visible_to(luke_runner, run) is False, "co-member is not entitled without assignment"


@pytest.mark.unit
def test_drain_pod_leaves_unassigned_issue_queued_for_other_members_runner(
    db, workspace, project, pod, backlog, alice, luke, luke_runner
):
    """Only Luke's runner is in the pod. The run must stay QUEUED, not leak."""
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[])
    run = _queued_run(workspace, pod, issue)

    assert matcher.drain_pod(pod) == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED
    assert run.runner_id is None


# ---------------------------------------------------------------------------
# Row 3: created by Luke, assigned to Luke -> only Luke's runner
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_other_members_issue_is_invisible_to_alices_runner(
    db, workspace, project, pod, backlog, alice, luke, alice_runner, luke_runner
):
    issue = _issue(workspace, project, backlog, creator=luke, assignees=[luke])
    run = _queued_run(workspace, pod, issue)

    assert _visible_to(luke_runner, run) is True
    assert _visible_to(alice_runner, run) is False, (
        "workspace admin standing must not let Alice's machine take Luke's work"
    )


@pytest.mark.unit
def test_drain_pod_does_not_give_lukes_issue_to_alices_runner(
    db, workspace, project, pod, backlog, alice, luke, alice_runner
):
    issue = _issue(workspace, project, backlog, creator=luke, assignees=[luke])
    run = _queued_run(workspace, pod, issue)

    assert matcher.drain_pod(pod) == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED
    assert run.runner_id is None


# ---------------------------------------------------------------------------
# Un-assignment withdraws the grant
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_removing_assignee_revokes_eligibility_while_still_queued(
    db, workspace, project, pod, backlog, alice, luke, luke_runner
):
    """Un-assignment must take Luke's runner back out of the eligible set.

    ``IssueAssignee`` is a ``SoftDeleteModel`` and the un-assign path only
    stamps ``deleted_at``; the eligibility reads therefore have to go through
    the through-model's live manager, not the ``Issue.assignees`` M2M (whose
    join ignores ``deleted_at`` and would grandfather every past assignee).
    """
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    assert _visible_to(luke_runner, run) is True

    # Exactly what the issue serializer does when assignees are edited.
    IssueAssignee.objects.filter(issue=issue).delete()
    assert IssueAssignee.all_objects.filter(issue=issue, assignee=luke).exists(), (
        "precondition: the row is soft-deleted, not removed"
    )

    assert _visible_to(luke_runner, run) is False
    assert matcher.drain_pod(pod) == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.QUEUED
    assert run.runner_id is None


@pytest.mark.unit
def test_preflight_ignores_soft_deleted_assignee(db, workspace, project, pod, backlog, alice, luke, luke_runner):
    """The creation-time preflight must agree with the dispatch gate."""
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    assert matcher.pod_has_runner_for_issue_principal(pod, issue, alice.id) is True

    IssueAssignee.objects.filter(issue=issue).delete()

    assert matcher.pod_has_runner_for_issue_principal(pod, issue, alice.id) is False


@pytest.mark.unit
def test_reassignment_after_removal_restores_eligibility(
    db, workspace, project, pod, backlog, alice, luke, luke_runner
):
    """Re-assigning leaves a soft-deleted row behind a live one; the live
    row must still be found (and must not be double-counted)."""
    issue = _issue(workspace, project, backlog, creator=alice, assignees=[luke])
    run = _queued_run(workspace, pod, issue)
    IssueAssignee.objects.filter(issue=issue).delete()
    IssueAssignee.objects.create(issue=issue, assignee=luke, workspace=workspace, project=project)

    assert IssueAssignee.all_objects.filter(issue=issue, assignee=luke).count() == 2
    assert _visible_to(luke_runner, run) is True
    assert matcher.drain_pod(pod) == 1
