# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import pytest
from crum import impersonate
from django.utils import timezone
from rest_framework.exceptions import ValidationError

from karma_team.app.serializers.project import ProjectSerializer
from karma_team.db.models import Issue, Project, State
from karma_team.db.models.issue_agent_ticker import (
    INFINITE_MAX_TICKS,
    IssueAgentTicker,
    TickerDisarmReason,
)


@pytest.mark.parametrize("max_ticks", [-1, 1, 10, 500])
def test_valid_max_ticks_pass_through(max_ticks):
    assert ProjectSerializer().validate_agent_default_max_ticks(max_ticks) == max_ticks


@pytest.mark.parametrize("max_ticks", [0, -2, -10])
def test_zero_and_other_negatives_are_rejected(max_ticks):
    with pytest.raises(ValidationError):
        ProjectSerializer().validate_agent_default_max_ticks(max_ticks)


@pytest.mark.parametrize(
    "validator",
    [
        "validate_agent_default_interval_seconds",
        "validate_agent_review_default_interval_seconds",
        "validate_agent_test_default_interval_seconds",
    ],
)
@pytest.mark.parametrize("interval", [60, 1800, 10800, 86400])
def test_cadence_at_or_above_the_scan_floor_passes(validator, interval):
    assert getattr(ProjectSerializer(), validator)(interval) == interval


@pytest.mark.parametrize(
    "validator",
    [
        "validate_agent_default_interval_seconds",
        "validate_agent_review_default_interval_seconds",
        "validate_agent_test_default_interval_seconds",
    ],
)
@pytest.mark.parametrize("interval", [59, 0, -1])
def test_cadence_below_the_scan_floor_is_rejected(validator, interval):
    with pytest.raises(ValidationError):
        getattr(ProjectSerializer(), validator)(interval)


@pytest.fixture
def ticking_project(db, workspace, create_user):
    with impersonate(create_user):
        return Project.objects.create(
            name="Ticking",
            identifier="TICK",
            workspace=workspace,
            created_by=create_user,
        )


@pytest.fixture
def ticking_states(ticking_project, create_user):
    with impersonate(create_user):
        return {
            "todo": State.objects.create(
                name="Todo",
                group="unstarted",
                project=ticking_project,
            ),
            "in_progress": State.objects.create(
                name="In Progress",
                group="started",
                project=ticking_project,
            ),
        }


def _issue_for_state(*, project, states, workspace, create_user, name, state):
    with impersonate(create_user):
        issue = Issue.objects.create(
            name=name,
            workspace=workspace,
            project=project,
            state=states["todo"],
            created_by=create_user,
        )
    if state != states["todo"]:
        Issue.all_objects.filter(pk=issue.pk).update(state=state)
        issue.refresh_from_db()
    return issue


def _set_ticking_enabled(project, enabled):
    serializer = ProjectSerializer(
        project,
        data={"agent_ticking_enabled": enabled},
        context={"workspace_id": project.workspace_id},
        partial=True,
    )
    serializer.is_valid(raise_exception=True)
    return serializer.save()


@pytest.mark.unit
def test_disabling_project_stops_automatic_work_but_keeps_human_pending(
    ticking_project,
    ticking_states,
    workspace,
    create_user,
):
    automatic_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="automatic",
        state=ticking_states["in_progress"],
    )
    agent_pending_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="agent pending",
        state=ticking_states["in_progress"],
    )
    human_pending_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="human pending",
        state=ticking_states["in_progress"],
    )
    marker = timezone.now()
    automatic = IssueAgentTicker.objects.create(
        issue=automatic_issue,
        enabled=True,
        next_run_at=marker,
        used=3,
    )
    agent_pending = IssueAgentTicker.objects.create(
        issue=agent_pending_issue,
        enabled=True,
        next_run_at=marker,
        pending_entry=True,
        pending_entry_free=False,
    )
    human_pending = IssueAgentTicker.objects.create(
        issue=human_pending_issue,
        enabled=True,
        next_run_at=marker,
        pending_entry=True,
        pending_entry_free=True,
        pending_entry_actor=create_user,
        pending_entry_trigger="run_ai",
    )

    _set_ticking_enabled(ticking_project, False)

    automatic.refresh_from_db()
    agent_pending.refresh_from_db()
    human_pending.refresh_from_db()
    assert automatic.enabled is False
    assert automatic.used == 3
    assert automatic.disarm_reason == TickerDisarmReason.NONE
    assert agent_pending.enabled is False
    assert agent_pending.pending_entry is False
    assert agent_pending.pending_entry_free is False
    assert human_pending.enabled is True
    assert human_pending.pending_entry is True
    assert human_pending.pending_entry_free is True
    assert human_pending.pending_entry_actor == create_user
    assert human_pending.pending_entry_trigger == "run_ai"


@pytest.mark.unit
def test_enabling_project_only_resumes_eligible_ticking_clocks(
    ticking_project,
    ticking_states,
    workspace,
    create_user,
):
    ticking_project.agent_ticking_enabled = False
    ticking_project.save(update_fields=["agent_ticking_enabled"])
    eligible_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="eligible",
        state=ticking_states["in_progress"],
    )
    user_disabled_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="user disabled",
        state=ticking_states["in_progress"],
    )
    capped_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="capped",
        state=ticking_states["in_progress"],
    )
    non_ticking_issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="todo",
        state=ticking_states["todo"],
    )
    stale_time = timezone.now() - timezone.timedelta(days=1)
    eligible = IssueAgentTicker.objects.create(
        issue=eligible_issue,
        enabled=False,
        next_run_at=stale_time,
    )
    user_disabled = IssueAgentTicker.objects.create(
        issue=user_disabled_issue,
        enabled=False,
        user_disabled=True,
        disarm_reason=TickerDisarmReason.USER_DISABLED,
        next_run_at=stale_time,
    )
    capped = IssueAgentTicker.objects.create(
        issue=capped_issue,
        enabled=False,
        used=ticking_project.agent_default_max_ticks,
        disarm_reason=TickerDisarmReason.POOL_SPENT,
        next_run_at=stale_time,
    )
    non_ticking = IssueAgentTicker.objects.create(
        issue=non_ticking_issue,
        enabled=False,
        disarm_reason=TickerDisarmReason.LEFT_TICKING_STATE,
        next_run_at=stale_time,
    )
    before = timezone.now()

    _set_ticking_enabled(ticking_project, True)

    eligible.refresh_from_db()
    user_disabled.refresh_from_db()
    capped.refresh_from_db()
    non_ticking.refresh_from_db()
    assert eligible.enabled is True
    assert eligible.disarm_reason == TickerDisarmReason.NONE
    assert eligible.next_run_at > before
    assert user_disabled.enabled is False
    assert user_disabled.disarm_reason == TickerDisarmReason.USER_DISABLED
    assert capped.enabled is False
    assert capped.disarm_reason == TickerDisarmReason.POOL_SPENT
    assert non_ticking.enabled is False
    assert non_ticking.disarm_reason == TickerDisarmReason.LEFT_TICKING_STATE


@pytest.mark.unit
def test_enabling_project_resumes_infinite_budget_clock(
    ticking_project,
    ticking_states,
    workspace,
    create_user,
):
    ticking_project.agent_ticking_enabled = False
    ticking_project.agent_default_max_ticks = INFINITE_MAX_TICKS
    ticking_project.save(update_fields=["agent_ticking_enabled", "agent_default_max_ticks"])
    issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="infinite",
        state=ticking_states["in_progress"],
    )
    ticker = IssueAgentTicker.objects.create(
        issue=issue,
        enabled=False,
        used=999_999,
        next_run_at=timezone.now() - timezone.timedelta(days=1),
    )

    _set_ticking_enabled(ticking_project, True)

    ticker.refresh_from_db()
    assert ticker.enabled is True
    assert ticker.next_run_at > timezone.now()


@pytest.mark.unit
def test_project_update_locks_project_before_tickers(
    ticking_project,
    ticking_states,
    workspace,
    create_user,
    monkeypatch,
):
    issue = _issue_for_state(
        project=ticking_project,
        states=ticking_states,
        workspace=workspace,
        create_user=create_user,
        name="lock order",
        state=ticking_states["in_progress"],
    )
    IssueAgentTicker.objects.create(issue=issue, enabled=True, next_run_at=timezone.now())
    lock_order = []
    original_project_lock = Project.objects.select_for_update
    original_ticker_lock = IssueAgentTicker.objects.select_for_update

    def project_lock(*args, **kwargs):
        lock_order.append("project")
        return original_project_lock(*args, **kwargs)

    def ticker_lock(*args, **kwargs):
        lock_order.append("ticker")
        return original_ticker_lock(*args, **kwargs)

    monkeypatch.setattr(Project.objects, "select_for_update", project_lock)
    monkeypatch.setattr(IssueAgentTicker.objects, "select_for_update", ticker_lock)

    _set_ticking_enabled(ticking_project, False)

    assert lock_order[:2] == ["project", "ticker"]
