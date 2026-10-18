# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Tests for moving a runner between pods via ``PATCH /api/runners/<id>/``.

The move is blocked while the runner has a non-terminal run *bound* to it —
one it is serving (``runner=``) or one reserved for it (``pinned_runner=``,
e.g. a queued/paused follow-up). A run's pod FK is immutable, so re-pointing
the runner would strand that run in the old pod. Unpinned queued runs are
not bound to the runner, so they don't block. See
``RunnerDetailEndpoint.patch``.
"""

from __future__ import annotations

import pytest
from django.urls import reverse
from django.utils import timezone

from karma_team.db.models import Project
from karma_team.runner.models import (
    AgentRun,
    AgentRunStatus,
    Pod,
    Runner,
    RunnerBindingSource,
    RunnerStatus,
)
from karma_team.runner.views.runners import RunnerDetailEndpoint


@pytest.fixture
def pod(project):
    return Pod.default_for_project(project)


@pytest.fixture
def other_pod(project, create_user):
    return Pod.objects.create(
        workspace=project.workspace,
        project=project,
        name=f"{project.identifier}_other",
        created_by=create_user,
    )


def _make_runner(user, workspace, pod, name="r1"):
    return Runner.objects.create(
        owner=user,
        workspace=workspace,
        pod=pod,
        name=name,
        status=RunnerStatus.ONLINE,
        last_heartbeat_at=timezone.now(),
    )


def _make_run(user, pod, *, runner=None, pinned_runner=None, status=AgentRunStatus.RUNNING):
    return AgentRun.objects.create(
        owner=user,
        created_by=user,
        workspace=pod.workspace,
        pod=pod,
        runner=runner,
        pinned_runner=pinned_runner,
        prompt="x",
        status=status,
    )


@pytest.mark.unit
def test_move_runner_to_another_pod_succeeds_when_idle(
    db, session_client, create_user, workspace, pod, other_pod
):
    runner = _make_runner(create_user, workspace, pod, "idle")
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(other_pod.id)}, format="json")
    assert resp.status_code == 200, resp.content
    runner.refresh_from_db()
    assert runner.pod_id == other_pod.id


@pytest.mark.unit
def test_auto_project_sync_runner_cannot_move_to_another_pod(
    db, session_client, create_user, workspace, pod, other_pod
):
    runner = _make_runner(create_user, workspace, pod, "auto-managed")
    runner.binding_source = RunnerBindingSource.AUTO_PROJECT_SYNC
    runner.save(update_fields=["binding_source", "updated_at"])

    response = session_client.patch(
        reverse("runner-detail", kwargs={"runner_id": runner.id}),
        {"pod": str(other_pod.id)},
        format="json",
    )

    assert response.status_code == 409, response.content
    assert response.json()["code"] == "auto_project_binding_managed"
    runner.refresh_from_db()
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_auto_project_sync_runner_allows_same_pod_noop_and_rename(
    db, session_client, create_user, workspace, pod
):
    runner = _make_runner(create_user, workspace, pod, "auto-managed-noop")
    runner.binding_source = RunnerBindingSource.AUTO_PROJECT_SYNC
    runner.save(update_fields=["binding_source", "updated_at"])

    response = session_client.patch(
        reverse("runner-detail", kwargs={"runner_id": runner.id}),
        {"pod": str(pod.id), "name": "auto-managed-renamed"},
        format="json",
    )

    assert response.status_code == 200, response.content
    runner.refresh_from_db()
    assert runner.pod_id == pod.id
    assert runner.name == "auto-managed-renamed"


@pytest.mark.unit
def test_move_runner_revalidates_target_project_after_initial_read(
    db, session_client, create_user, workspace, pod, monkeypatch
):
    """A target project deleted during PATCH setup cannot receive a runner."""
    target_project = Project.objects.create(
        name="Move race target",
        identifier="MOVRACE",
        workspace=workspace,
        created_by=create_user,
    )
    target_pod = Pod.default_for_project(target_project)
    runner = _make_runner(create_user, workspace, pod, "move-race")
    original_get_runner = RunnerDetailEndpoint._get_runner

    def get_runner_then_delete_target(view, request, runner_id):
        result = original_get_runner(view, request, runner_id)
        Project.all_objects.filter(pk=target_project.id).update(
            deleted_at=timezone.now()
        )
        return result

    monkeypatch.setattr(
        RunnerDetailEndpoint,
        "_get_runner",
        get_runner_then_delete_target,
    )

    response = session_client.patch(
        reverse("runner-detail", kwargs={"runner_id": runner.id}),
        {"pod": str(target_pod.id)},
        format="json",
    )

    assert response.status_code == 400, response.content
    runner.refresh_from_db()
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_move_runner_blocked_while_serving_active_run(
    db, session_client, create_user, workspace, pod, other_pod
):
    runner = _make_runner(create_user, workspace, pod, "busy")
    _make_run(create_user, runner.pod, runner=runner, status=AgentRunStatus.RUNNING)
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(other_pod.id)}, format="json")
    assert resp.status_code == 409, resp.content
    assert resp.json().get("code") == "runner_busy"
    runner.refresh_from_db()
    # Pod is unchanged.
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_move_runner_blocked_with_queued_pinned_run(
    db, session_client, create_user, workspace, pod, other_pod
):
    """A queued run reserved for this runner (pinned, runner FK still null)
    is pending work bound to it — moving the runner would strand it."""
    runner = _make_runner(create_user, workspace, pod, "pinned")
    _make_run(
        create_user, runner.pod, pinned_runner=runner, status=AgentRunStatus.QUEUED
    )
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(other_pod.id)}, format="json")
    assert resp.status_code == 409, resp.content
    assert resp.json().get("code") == "runner_busy"
    runner.refresh_from_db()
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_unpinned_queued_run_does_not_block_move(
    db, session_client, create_user, workspace, pod, other_pod
):
    """A queued run in the pod that is NOT bound to this runner (no runner,
    no pinned_runner) can be served by any runner, so it doesn't block."""
    runner = _make_runner(create_user, workspace, pod, "unpinned")
    _make_run(create_user, runner.pod, status=AgentRunStatus.QUEUED)
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(other_pod.id)}, format="json")
    assert resp.status_code == 200, resp.content
    runner.refresh_from_db()
    assert runner.pod_id == other_pod.id


@pytest.mark.unit
def test_resend_same_pod_allowed_while_serving_active_run(
    db, session_client, create_user, workspace, pod
):
    """Re-sending the runner's current pod is a no-op, allowed even mid-run."""
    runner = _make_runner(create_user, workspace, pod, "busy-noop")
    _make_run(create_user, runner.pod, runner=runner, status=AgentRunStatus.RUNNING)
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(pod.id)}, format="json")
    assert resp.status_code == 200, resp.content
    runner.refresh_from_db()
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_paused_run_bound_to_runner_blocks_move(
    db, session_client, create_user, workspace, pod, other_pod
):
    """A paused run is non-terminal and still bound to its runner; it resumes
    on that runner, so moving the runner to another pod would strand it."""
    runner = _make_runner(create_user, workspace, pod, "paused")
    _make_run(
        create_user, runner.pod, runner=runner,
        status=AgentRunStatus.PAUSED_AWAITING_INPUT,
    )
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"pod": str(other_pod.id)}, format="json")
    assert resp.status_code == 409, resp.content
    assert resp.json().get("code") == "runner_busy"
    runner.refresh_from_db()
    assert runner.pod_id == pod.id


@pytest.mark.unit
def test_rename_allowed_while_serving_active_run(
    db, session_client, create_user, workspace, pod
):
    """The busy guard is scoped to pod moves; a rename still goes through."""
    runner = _make_runner(create_user, workspace, pod, "busy-rename")
    _make_run(create_user, runner.pod, runner=runner, status=AgentRunStatus.RUNNING)
    url = reverse("runner-detail", kwargs={"runner_id": runner.id})
    resp = session_client.patch(url, {"name": "renamed"}, format="json")
    assert resp.status_code == 200, resp.content
    runner.refresh_from_db()
    assert runner.name == "renamed"
