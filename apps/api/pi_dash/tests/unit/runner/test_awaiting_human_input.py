# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Runs the runner stopped because the agent kept asking a human a question.

The runner reports ``RunFailed{reason: "awaiting_human_input"}`` with the
question in ``detail``. The cloud must (1) record it as FAILED tagged with
``error_code`` and (2) have the ticker wait for a human instead of
re-dispatching into the same question loop.
"""

from __future__ import annotations

import pytest
from crum import impersonate
from django.utils import timezone

from pi_dash.db.models import Issue, State
from pi_dash.db.models.issue_agent_ticker import IssueAgentTicker, TickerDisarmReason
from pi_dash.orchestration import scheduling
from pi_dash.orchestration.scheduling import TickerEvent
from pi_dash.prompting.seed import seed_default_template
from pi_dash.runner.models import (
    AWAITING_HUMAN_INPUT_ERROR_CODE,
    AgentRun,
    AgentRunStatus,
    Pod,
    Runner,
    RunnerStatus,
)
from pi_dash.runner.services import tokens


@pytest.fixture
def pod(project):
    return Pod.default_for_project(project)


@pytest.fixture
def runner(db, create_user, workspace, pod):
    return Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        pod=pod,
        name="agentR",
        status=RunnerStatus.ONLINE,
        last_heartbeat_at=timezone.now(),
        refresh_token_generation=1,
        enrolled_at=timezone.now(),
    )


@pytest.fixture
def runner_token(runner):
    return tokens.mint_access_token(
        runner_id=str(runner.id),
        user_id=str(runner.owner_id),
        workspace_id=str(runner.workspace_id),
        rtg=1,
    ).raw


@pytest.fixture
def running_run(db, create_user, workspace, pod, runner):
    return AgentRun.objects.create(
        owner=create_user,
        created_by=create_user,
        workspace=workspace,
        pod=pod,
        runner=runner,
        prompt="x",
        status=AgentRunStatus.RUNNING,
        started_at=timezone.now(),
    )


def _fail(api_client, token, run, reason):
    return api_client.post(
        f"/api/v1/runner/runs/{run.id}/fail/",
        {"reason": reason, "detail": "Last question:\nProceed?"},
        format="json",
        HTTP_AUTHORIZATION=f"Bearer {token}",
    )


# ---------------------------------------------------------------------------
# RunFailed endpoint
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_fail_endpoint_tags_awaiting_human_input(db, api_client, runner_token, running_run):
    resp = _fail(api_client, runner_token, running_run, AWAITING_HUMAN_INPUT_ERROR_CODE)
    assert resp.status_code == 200, resp.data
    running_run.refresh_from_db()
    assert running_run.status == AgentRunStatus.FAILED
    assert running_run.error_code == AWAITING_HUMAN_INPUT_ERROR_CODE
    assert "Proceed?" in running_run.error


@pytest.mark.unit
def test_fail_endpoint_leaves_other_reasons_untagged(db, api_client, runner_token, running_run):
    resp = _fail(api_client, runner_token, running_run, "timeout")
    assert resp.status_code == 200, resp.data
    running_run.refresh_from_db()
    assert running_run.status == AgentRunStatus.FAILED
    assert running_run.error_code == ""


# ---------------------------------------------------------------------------
# Ticker reaction to the tagged failure
# ---------------------------------------------------------------------------


@pytest.fixture
def in_progress_issue(db, workspace, project, create_user):
    seed_default_template()
    with impersonate(create_user):
        todo = State.objects.create(name="Todo", project=project, group="unstarted")
        in_progress = State.objects.create(name="In Progress", project=project, group="started")
        issue = Issue.objects.create(
            name="Logistics portal",
            workspace=workspace,
            project=project,
            state=todo,
            created_by=create_user,
        )
    # Move without firing the post_save signal, so the test owns the ticker.
    Issue.all_objects.filter(pk=issue.pk).update(state=in_progress)
    issue.refresh_from_db()
    return issue


def _failed_run(issue, runner, *, error_code=""):
    return AgentRun.objects.create(
        workspace=issue.workspace,
        owner=runner.owner,
        pod=runner.pod,
        work_item=issue,
        runner=runner,
        status=AgentRunStatus.FAILED,
        phase_kind="coding-task",
        error_code=error_code,
        prompt="x",
        started_at=timezone.now(),
        ended_at=timezone.now(),
    )


@pytest.mark.unit
def test_awaiting_human_input_failure_stops_the_clock(db, in_progress_issue, runner):
    IssueAgentTicker.objects.create(issue=in_progress_issue, enabled=True, next_run_at=timezone.now())
    run = _failed_run(in_progress_issue, runner, error_code=AWAITING_HUMAN_INPUT_ERROR_CODE)

    decision = scheduling.reconcile(in_progress_issue, TickerEvent.run_ended(run))

    assert decision.reason == "waiting_on_human:stopped"
    assert decision.ticker.enabled is False
    assert decision.ticker.disarm_reason == TickerDisarmReason.TERMINAL_SIGNAL


@pytest.mark.unit
def test_other_failures_keep_ticking(db, in_progress_issue, runner):
    """A crash says nothing about the stage; the next tick retries."""
    IssueAgentTicker.objects.create(issue=in_progress_issue, enabled=True, next_run_at=None)
    run = _failed_run(in_progress_issue, runner)

    decision = scheduling.reconcile(in_progress_issue, TickerEvent.run_ended(run))

    assert decision.reason == "progressed:keep-ticking"
    assert decision.ticker.enabled is True
