# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Recovery of runs stranded on a runner that stopped heartbeating."""

from __future__ import annotations

from datetime import timedelta
from unittest.mock import patch
from uuid import uuid4

import pytest
from django.test import override_settings
from django.utils import timezone

from karma_team.core.agent_execution import AgentExecutorKind
from karma_team.db.models import Issue, ProjectMember, State, User, WorkspaceMember
from karma_team.db.models.issue import IssueAssignee
from karma_team.orchestration.service import _pinned_runner_for
from karma_team.runner import tasks
from karma_team.runner.models import (
    AgentRun,
    AgentRunStatus,
    Pod,
    Runner,
    RunnerStatus,
)
from karma_team.runner.services import session_service


@pytest.fixture
def pod(project):
    return Pod.default_for_project(project)


@pytest.fixture(autouse=True)
def _project_membership(workspace, project, create_user):
    ProjectMember.objects.get_or_create(
        workspace=workspace,
        project=project,
        member=create_user,
        defaults={"role": 20},
    )


@pytest.fixture(autouse=True)
def _stub_send_to_runner():
    with patch("karma_team.runner.services.pubsub.send_to_runner") as mock:
        yield mock


@pytest.fixture(autouse=True)
def _stub_terminal_effects():
    """Terminal hooks publish to Celery; these tests focus on run recovery."""
    with patch("karma_team.runner.services.agent_run_finalization._publish_effects"):
        yield


@pytest.fixture(autouse=True)
def _run_on_commit_immediately():
    with patch("django.db.transaction.on_commit", side_effect=lambda fn, **kw: fn()):
        yield


def _make_runner(user, workspace, pod, name, *, silent_for_s=1, status=RunnerStatus.ONLINE):
    return Runner.objects.create(
        owner=user,
        workspace=workspace,
        pod=pod,
        name=name,
        status=status,
        last_heartbeat_at=(None if silent_for_s is None else timezone.now() - timedelta(seconds=silent_for_s)),
    )


def _make_run(user, workspace, pod, *, status=AgentRunStatus.QUEUED, age_s=0, **fields):
    run = AgentRun.objects.create(
        owner=user,
        created_by=user,
        workspace=workspace,
        pod=pod,
        prompt="x",
        status=status,
        **fields,
    )
    then = timezone.now() - timedelta(seconds=age_s)
    stamps = {"created_at": then}
    if status != AgentRunStatus.QUEUED:
        stamps["assigned_at"] = then
    AgentRun.objects.filter(pk=run.pk).update(**stamps)
    run.refresh_from_db()
    return run


@pytest.mark.unit
@pytest.mark.parametrize("status", [RunnerStatus.ONLINE, RunnerStatus.BUSY])
def test_pin_kept_for_fresh_available_runner(db, create_user, workspace, pod, status):
    runner = _make_runner(create_user, workspace, pod, "alive", status=status)
    parent = _make_run(create_user, workspace, pod, status=AgentRunStatus.COMPLETED, runner=runner)

    assert _pinned_runner_for(parent, pod) == runner


@pytest.mark.unit
@pytest.mark.parametrize(
    "status,silent_for_s",
    [
        (RunnerStatus.OFFLINE, 3600),
        (RunnerStatus.REVOKED, 1),
        (RunnerStatus.ONLINE, 3600),
        (RunnerStatus.ONLINE, None),
        (RunnerStatus.BUSY, 3600),
    ],
)
def test_no_pin_to_unavailable_runner(db, create_user, workspace, pod, status, silent_for_s):
    runner = _make_runner(create_user, workspace, pod, "gone", status=status, silent_for_s=silent_for_s)
    parent = _make_run(create_user, workspace, pod, status=AgentRunStatus.FAILED, runner=runner)

    assert _pinned_runner_for(parent, pod) is None


@pytest.mark.unit
def test_release_pin_hands_run_to_another_runner(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    alive = _make_runner(create_user, workspace, pod, "alive")
    run = _make_run(create_user, workspace, pod, pinned_runner=gone, age_s=3600)

    assert tasks.release_pins_on_offline_runners() == 1

    run.refresh_from_db()
    assert run.pinned_runner_id is None
    assert run.status == AgentRunStatus.ASSIGNED
    assert run.runner_id == alive.id


@pytest.mark.unit
def test_release_pin_clears_parent_thread_id(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    parent = _make_run(
        create_user,
        workspace,
        pod,
        status=AgentRunStatus.FAILED,
        runner=gone,
        thread_id="thread-1",
    )
    run = _make_run(create_user, workspace, pod, pinned_runner=gone, parent_run=parent, age_s=3600)

    assert tasks.release_pins_on_offline_runners() == 1

    run.refresh_from_db()
    parent.refresh_from_db()
    assert run.pinned_runner_id is None
    assert run.status == AgentRunStatus.QUEUED
    assert parent.thread_id == ""


@pytest.mark.unit
def test_release_pin_waits_out_grace_and_skips_managed_runs(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    young = _make_run(create_user, workspace, pod, pinned_runner=gone, age_s=60)
    managed = _make_run(
        create_user,
        workspace,
        pod,
        pinned_runner=gone,
        executor_kind=AgentExecutorKind.MANAGED_RUNNER,
        age_s=3600,
    )

    assert tasks.release_pins_on_offline_runners() == 0

    young.refresh_from_db()
    managed.refresh_from_db()
    assert young.pinned_runner_id == gone.id
    assert managed.pinned_runner_id == gone.id


@pytest.mark.unit
@override_settings(RUNNER_OFFLINE_PIN_RELEASE_SECS=0)
def test_release_pin_can_be_disabled(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    run = _make_run(create_user, workspace, pod, pinned_runner=gone, age_s=3600)

    assert tasks.release_pins_on_offline_runners() == 0
    run.refresh_from_db()
    assert run.pinned_runner_id == gone.id


@pytest.mark.unit
def test_release_pin_preserves_assignee_runner_eligibility(db, create_user, workspace, project, pod):
    suffix = uuid4().hex[:8]
    assignee = User.objects.create(email=f"assignee-{suffix}@example.com", username=f"assignee_{suffix}")
    WorkspaceMember.objects.create(workspace=workspace, member=assignee, role=15)
    ProjectMember.objects.create(workspace=workspace, project=project, member=assignee, role=15)

    state = State.objects.create(name="In Progress", project=project, group="started")
    issue = Issue.objects.create(
        project=project,
        workspace=workspace,
        name="Assigned recovery",
        state=state,
        created_by=create_user,
    )
    IssueAssignee.objects.create(
        issue=issue,
        assignee=assignee,
        workspace=workspace,
        project=project,
    )
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    creator_runner = _make_runner(create_user, workspace, pod, "creator")
    assignee_runner = _make_runner(assignee, workspace, pod, "assignee")
    Runner.objects.filter(pk=creator_runner.pk).update(last_heartbeat_at=timezone.now())
    run = _make_run(create_user, workspace, pod, work_item=issue, pinned_runner=gone, age_s=3600)

    assert tasks.release_pins_on_offline_runners() == 1

    run.refresh_from_db()
    assert run.status == AgentRunStatus.ASSIGNED
    assert run.runner_id == assignee_runner.id
    assert run.owner_id == assignee.id


@pytest.mark.unit
def test_release_pin_rechecks_heartbeat_under_runner_lock(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "returned", status=RunnerStatus.OFFLINE, silent_for_s=3600)
    run = _make_run(create_user, workspace, pod, pinned_runner=gone, age_s=3600)

    from django.db.models.query import QuerySet

    original_select_for_update = QuerySet.select_for_update

    def runner_returns_before_lock(queryset, *args, **kwargs):
        if queryset.model is Runner:
            Runner.objects.filter(pk=gone.pk).update(last_heartbeat_at=timezone.now())
        return original_select_for_update(queryset, *args, **kwargs)

    with patch.object(QuerySet, "select_for_update", runner_returns_before_lock):
        assert tasks.release_pins_on_offline_runners() == 0

    run.refresh_from_db()
    assert run.pinned_runner_id == gone.id


@pytest.mark.unit
@pytest.mark.parametrize(
    "status",
    [
        AgentRunStatus.ASSIGNED,
        AgentRunStatus.RUNNING,
        AgentRunStatus.AWAITING_APPROVAL,
        AgentRunStatus.AWAITING_REAUTH,
    ],
)
def test_fail_active_run_on_long_silent_runner(db, create_user, workspace, pod, status):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(create_user, workspace, pod, status=status, runner=gone, age_s=7200)

    assert tasks.fail_runs_on_offline_runners() == 1

    run.refresh_from_db()
    assert run.status == AgentRunStatus.FAILED
    assert run.error_code == session_service.RUNNER_OFFLINE_ERROR_CODE
    assert run.ended_at is not None


@pytest.mark.unit
def test_fail_run_skips_managed_runs(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(
        create_user,
        workspace,
        pod,
        status=AgentRunStatus.RUNNING,
        runner=gone,
        executor_kind=AgentExecutorKind.MANAGED_RUNNER,
        age_s=7200,
    )

    assert tasks.fail_runs_on_offline_runners() == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.RUNNING


@pytest.mark.unit
def test_fail_run_rechecks_heartbeat_under_runner_lock(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(create_user, workspace, pod, status=AgentRunStatus.RUNNING, runner=gone, age_s=7200)
    real = session_service._finish_unclaimed_runs

    # Simulate the daemon polling after the sweep lists it but before the
    # sweep acquires the runner row lock and re-checks its heartbeat.
    def _runner_returns_then_lock(qs_method):
        def wrapped(self, *args, **kwargs):
            Runner.objects.filter(pk=gone.pk).update(last_heartbeat_at=timezone.now())
            return qs_method(self, *args, **kwargs)

        return wrapped

    from django.db.models.query import QuerySet

    with patch.object(QuerySet, "select_for_update", _runner_returns_then_lock(QuerySet.select_for_update)):
        with patch.object(session_service, "_finish_unclaimed_runs", wraps=real) as finish:
            assert tasks.fail_runs_on_offline_runners() == 0

    finish.assert_not_called()
    run.refresh_from_db()
    assert run.status == AgentRunStatus.RUNNING


@pytest.mark.unit
@pytest.mark.parametrize("silent_for_s", [1, 2700])
def test_fail_run_ignores_runner_within_grace(db, create_user, workspace, pod, silent_for_s):
    runner = _make_runner(create_user, workspace, pod, "available", silent_for_s=silent_for_s)
    run = _make_run(create_user, workspace, pod, status=AgentRunStatus.RUNNING, runner=runner, age_s=7200)

    assert tasks.fail_runs_on_offline_runners() == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.RUNNING


@pytest.mark.unit
def test_pending_cancellation_is_finalized_and_stamped(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(
        create_user,
        workspace,
        pod,
        status=AgentRunStatus.CANCEL_REQUESTED,
        runner=gone,
        age_s=7200,
    )

    assert tasks.fail_runs_on_offline_runners() == 1

    run.refresh_from_db()
    assert run.status == AgentRunStatus.CANCELLED
    assert run.error_code == session_service.RUNNER_OFFLINE_ERROR_CODE
    assert run.ended_at is not None


@pytest.mark.unit
def test_pending_cancellation_does_not_overwrite_racing_completion(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(
        create_user,
        workspace,
        pod,
        status=AgentRunStatus.CANCEL_REQUESTED,
        runner=gone,
        age_s=7200,
    )
    completed_at = timezone.now() - timedelta(seconds=5)

    from karma_team.runner.services import agent_run_finalization

    real_finalize = agent_run_finalization.finalize_agent_run

    def completion_wins(run_id, *args, **kwargs):
        AgentRun.objects.filter(pk=run_id).update(
            status=AgentRunStatus.COMPLETED,
            ended_at=completed_at,
            error_code="",
        )
        return real_finalize(run_id, *args, **kwargs)

    with patch.object(agent_run_finalization, "finalize_agent_run", side_effect=completion_wins):
        assert tasks.fail_runs_on_offline_runners() == 0

    run.refresh_from_db()
    assert run.status == AgentRunStatus.COMPLETED
    assert run.ended_at == completed_at
    assert run.error_code == ""


@pytest.mark.unit
@override_settings(RUNNER_OFFLINE_RUN_FAIL_SECS=0)
def test_fail_run_can_be_disabled(db, create_user, workspace, pod):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(create_user, workspace, pod, status=AgentRunStatus.RUNNING, runner=gone, age_s=7200)

    assert tasks.fail_runs_on_offline_runners() == 0
    run.refresh_from_db()
    assert run.status == AgentRunStatus.RUNNING


@pytest.mark.unit
@pytest.mark.parametrize(
    "initial_status,terminal_status,reason",
    [
        (AgentRunStatus.RUNNING, AgentRunStatus.FAILED, "run_already_failed"),
        (AgentRunStatus.CANCEL_REQUESTED, AgentRunStatus.CANCELLED, "run_already_cancelled"),
    ],
)
def test_returning_daemon_is_told_to_stop_finalized_run(
    db,
    create_user,
    workspace,
    pod,
    _stub_send_to_runner,
    initial_status,
    terminal_status,
    reason,
):
    gone = _make_runner(create_user, workspace, pod, "gone", status=RunnerStatus.OFFLINE, silent_for_s=7200)
    run = _make_run(create_user, workspace, pod, status=initial_status, runner=gone, age_s=7200)
    assert tasks.fail_runs_on_offline_runners() == 1
    run.refresh_from_db()
    assert run.status == terminal_status
    _stub_send_to_runner.reset_mock()

    session_service.reap_stale_busy_runs(gone, {"in_flight_run": str(run.id)})

    _stub_send_to_runner.assert_called_once_with(
        gone.id,
        {"v": 1, "type": "cancel", "run_id": str(run.id), "reason": reason},
    )


@pytest.mark.unit
def test_poll_does_not_cancel_run_failed_for_another_reason(db, create_user, workspace, pod, _stub_send_to_runner):
    runner = _make_runner(create_user, workspace, pod, "alive")
    run = _make_run(
        create_user,
        workspace,
        pod,
        status=AgentRunStatus.FAILED,
        runner=runner,
        error_code="heartbeat_reaped",
    )

    session_service.reap_stale_busy_runs(runner, {"in_flight_run": str(run.id)})

    _stub_send_to_runner.assert_not_called()
