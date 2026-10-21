# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from __future__ import annotations

from datetime import timedelta
from unittest import mock

import pytest
from crum import impersonate
from django.utils import timezone

from karma_team.bgtasks.agent_ticker import fire_tick
from karma_team.db.models import Issue, IssueRelation, Project, ProjectMember, State
from karma_team.db.models.issue_agent_ticker import IssueAgentTicker
from karma_team.orchestration import scheduling
from karma_team.orchestration.blocking import is_blocked, unresolved_blockers
from karma_team.prompting.seed import seed_default_template
from karma_team.runner.models import AgentRun, AgentRunStatus, Pod, Runner, RunnerStatus


@pytest.fixture
def seeded(db):
    seed_default_template()


@pytest.fixture
def project(db, workspace, create_user):
    with impersonate(create_user):
        return Project.objects.create(
            name="Dependency project",
            identifier="DEP",
            workspace=workspace,
            created_by=create_user,
        )


@pytest.fixture
def states(project, create_user):
    with impersonate(create_user):
        return {
            "backlog": State.objects.create(name="Backlog", project=project, group="backlog"),
            "todo": State.objects.create(name="Todo", project=project, group="unstarted"),
            "in_progress": State.objects.create(name="In Progress", project=project, group="started"),
            "in_review": State.objects.create(name="In Review", project=project, group="review"),
            "in_test": State.objects.create(name="In Test", project=project, group="test"),
            "done": State.objects.create(name="Done", project=project, group="completed"),
            "cancelled": State.objects.create(name="Cancelled", project=project, group="cancelled"),
        }


@pytest.fixture
def runner_for_project(workspace, project, create_user):
    ProjectMember.objects.get_or_create(
        workspace=workspace,
        project=project,
        member=create_user,
        defaults={"role": 20},
    )
    return Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        pod=Pod.default_for_project(project),
        name="dependency-runner",
        status=RunnerStatus.ONLINE,
        last_heartbeat_at=timezone.now(),
    )


@pytest.fixture(autouse=True)
def orchestration_side_effects(monkeypatch):
    from karma_team.runner.services import matcher

    monkeypatch.setattr(matcher, "drain_pod_by_id", mock.Mock())
    monkeypatch.setattr("django.db.transaction.on_commit", lambda fn, **kwargs: fn())
    monkeypatch.setattr("karma_team.db.mixins.soft_delete_related_objects.delay", mock.Mock())


def _make_issue(*, name, state, workspace, project, create_user):
    with impersonate(create_user):
        return Issue.objects.create(
            name=name,
            workspace=workspace,
            project=project,
            state=state,
            created_by=create_user,
        )


def _set_state_without_signal(issue, state):
    Issue.all_objects.filter(pk=issue.pk).update(state=state)
    issue.refresh_from_db()


def _move_issue(issue, state, actor):
    with impersonate(actor):
        issue.state = state
        issue.save(update_fields=["state", "updated_at"])
    issue.refresh_from_db()


def _block(blocked_issue, blocker, actor):
    with impersonate(actor):
        return IssueRelation.objects.create(
            issue=blocked_issue,
            related_issue=blocker,
            relation_type="blocked_by",
            project=blocked_issue.project,
            workspace=blocked_issue.workspace,
            created_by=actor,
        )


def _blocked_pair(*, workspace, project, states, create_user):
    blocker = _make_issue(
        name="Blocker",
        state=states["todo"],
        workspace=workspace,
        project=project,
        create_user=create_user,
    )
    blocked_issue = _make_issue(
        name="Blocked",
        state=states["todo"],
        workspace=workspace,
        project=project,
        create_user=create_user,
    )
    _set_state_without_signal(blocker, states["in_progress"])
    relation = _block(blocked_issue, blocker, create_user)
    return blocker, blocked_issue, relation


@pytest.mark.unit
def test_blocked_issue_entering_in_progress_does_not_create_run_or_pending_entry(
    seeded, workspace, project, states, create_user, runner_for_project
):
    _blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )

    _move_issue(blocked_issue, states["in_progress"], create_user)

    assert AgentRun.objects.filter(work_item=blocked_issue).exists() is False
    assert IssueAgentTicker.objects.filter(issue=blocked_issue).exists() is False
    assert is_blocked(blocked_issue) is True


@pytest.mark.unit
def test_in_test_blocker_wakes_an_in_progress_issue(
    seeded, workspace, project, states, create_user, runner_for_project
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _move_issue(blocked_issue, states["in_progress"], create_user)

    _move_issue(blocker, states["in_test"], create_user)

    run = AgentRun.objects.get(work_item=blocked_issue)
    assert run.status == AgentRunStatus.QUEUED
    assert run.trigger == "state_transition"
    assert is_blocked(blocked_issue) is False


@pytest.mark.unit
@pytest.mark.parametrize("blocked_state", ["todo", "backlog", "in_review", "in_test"])
def test_released_blocker_only_auto_wakes_started_issues(
    blocked_state,
    seeded,
    workspace,
    project,
    states,
    create_user,
    runner_for_project,
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _set_state_without_signal(blocked_issue, states[blocked_state])

    _move_issue(blocker, states["in_test"], create_user)

    assert AgentRun.objects.filter(work_item=blocked_issue).exists() is False


@pytest.mark.unit
@pytest.mark.parametrize("release_state", ["in_test", "done", "cancelled"])
def test_test_and_terminal_states_release_a_blocked_issue(
    release_state,
    seeded,
    workspace,
    project,
    states,
    create_user,
    runner_for_project,
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _move_issue(blocked_issue, states["in_progress"], create_user)

    _move_issue(blocker, states[release_state], create_user)

    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1


@pytest.mark.unit
def test_all_blockers_must_release_before_auto_wake(
    seeded, workspace, project, states, create_user, runner_for_project
):
    blocker_a, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    blocker_b = _make_issue(
        name="Second blocker",
        state=states["todo"],
        workspace=workspace,
        project=project,
        create_user=create_user,
    )
    _set_state_without_signal(blocker_b, states["in_progress"])
    _block(blocked_issue, blocker_b, create_user)
    _move_issue(blocked_issue, states["in_progress"], create_user)

    _move_issue(blocker_a, states["in_test"], create_user)
    assert AgentRun.objects.filter(work_item=blocked_issue).exists() is False
    assert unresolved_blockers(blocked_issue).count() == 1

    _move_issue(blocker_b, states["done"], create_user)
    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1


@pytest.mark.unit
def test_fire_tick_blocked_does_not_spend_budget(seeded, workspace, project, states, create_user, runner_for_project):
    _blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _set_state_without_signal(blocked_issue, states["in_progress"])
    AgentRun.objects.create(
        workspace=workspace,
        created_by=create_user,
        pod=runner_for_project.pod,
        work_item=blocked_issue,
        status=AgentRunStatus.PAUSED_AWAITING_INPUT,
        prompt="prior",
    )
    ticker = scheduling.arm_ticker(blocked_issue)
    ticker.used = 4
    ticker.next_run_at = timezone.now() - timedelta(seconds=1)
    ticker.save(update_fields=["used", "next_run_at", "updated_at"])

    assert fire_tick(str(ticker.pk)) is False

    ticker.refresh_from_db()
    assert ticker.used == 4
    assert ticker.next_run_at <= timezone.now()
    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1


@pytest.mark.unit
def test_run_ai_blocked_returns_reason_and_rolls_back_ticker(seeded, workspace, project, states, create_user):
    _blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _set_state_without_signal(blocked_issue, states["in_progress"])
    ticker = scheduling.arm_ticker(blocked_issue)
    ticker.used = 3
    ticker.next_run_at = timezone.now() + timedelta(hours=2)
    ticker.save(update_fields=["used", "next_run_at", "updated_at"])
    original_next_run_at = ticker.next_run_at

    run, reason = scheduling.run_ai_for_human(blocked_issue, actor=create_user)

    assert run is None
    assert reason == scheduling.RUN_AI_BLOCKED
    ticker.refresh_from_db()
    assert ticker.used == 3
    assert ticker.next_run_at == original_next_run_at
    assert AgentRun.objects.filter(work_item=blocked_issue).exists() is False


@pytest.mark.unit
def test_removing_blocked_by_relation_wakes_in_progress_issue(
    seeded,
    session_client,
    workspace,
    project,
    states,
    create_user,
    runner_for_project,
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _move_issue(blocked_issue, states["in_progress"], create_user)

    list_response = session_client.get(
        f"/api/workspaces/{workspace.slug}/projects/{project.pk}/issues/{blocked_issue.pk}/issue-relation/"
    )
    assert list_response.status_code == 200, list_response.data
    assert list_response.data["blocked_by"][0]["id"] == blocker.pk
    assert list_response.data["blocked_by"][0]["state__group"] == "started"

    remove_response = session_client.post(
        f"/api/workspaces/{workspace.slug}/projects/{project.pk}/issues/{blocked_issue.pk}/remove-relation/",
        {"relation_type": "blocked_by", "related_issue": str(blocker.pk)},
        format="json",
    )

    assert remove_response.status_code == 204, remove_response.data
    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1


@pytest.mark.unit
def test_moving_issue_to_in_progress_after_earlier_release_dispatches_normally(
    seeded, workspace, project, states, create_user, runner_for_project
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _move_issue(blocker, states["in_test"], create_user)
    assert AgentRun.objects.filter(work_item=blocked_issue).exists() is False

    _move_issue(blocked_issue, states["in_progress"], create_user)

    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1


@pytest.mark.unit
def test_soft_deleted_blocker_wakes_in_progress_issue(
    seeded, workspace, project, states, create_user, runner_for_project
):
    blocker, blocked_issue, _relation = _blocked_pair(
        workspace=workspace,
        project=project,
        states=states,
        create_user=create_user,
    )
    _move_issue(blocked_issue, states["in_progress"], create_user)

    blocker.delete()

    assert AgentRun.objects.filter(work_item=blocked_issue).count() == 1
    assert is_blocked(blocked_issue) is False
