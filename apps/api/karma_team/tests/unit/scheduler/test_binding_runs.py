# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Project scheduler binding detail and run-history tests."""

from __future__ import annotations

from datetime import timedelta

import pytest
from crum import impersonate
from django.contrib.auth import get_user_model
from django.utils import timezone
from rest_framework.test import APIRequestFactory, force_authenticate

from karma_team.app.views.scheduler.views import (
    ProjectSchedulerBindingDetailEndpoint,
    ProjectSchedulerBindingListEndpoint,
    ProjectSchedulerBindingRunsEndpoint,
)
from karma_team.db.models import (
    Issue,
    Project,
    ProjectMember,
    Scheduler,
    SchedulerBinding,
    WorkspaceMember,
)
from karma_team.runner.models import AgentRun, DevMachine, Pod, Runner
from karma_team.runner.views.runs import AgentRunDetailEndpoint, AgentRunListEndpoint


User = get_user_model()


@pytest.fixture
def scheduler_project(project, workspace, create_user):
    ProjectMember.objects.get_or_create(
        project=project,
        workspace=workspace,
        member=create_user,
        defaults={"role": 20},
    )
    return project


@pytest.fixture
def scheduler(workspace, create_user):
    with impersonate(create_user):
        return Scheduler.objects.create(
            workspace=workspace,
            slug="history-audit",
            name="History Audit",
            prompt="Audit the project.",
            color="#10b981",
        )


@pytest.fixture
def binding(scheduler, scheduler_project, workspace, create_user):
    with impersonate(create_user):
        return SchedulerBinding.objects.create(
            scheduler=scheduler,
            project=scheduler_project,
            workspace=workspace,
            dtstart=timezone.now() - timedelta(days=1),
            rrule="FREQ=DAILY",
            tzid="UTC",
            enabled=True,
            actor=create_user,
        )


@pytest.fixture
def default_pod(scheduler_project):
    return Pod.objects.get(project=scheduler_project, is_default=True)


def _make_binding(scheduler_project, workspace, user, slug="other-audit"):
    with impersonate(user):
        scheduler = Scheduler.objects.create(
            workspace=workspace,
            slug=slug,
            name="Other Audit",
            prompt="Report.",
            color="#3b82f6",
        )
        return SchedulerBinding.objects.create(
            scheduler=scheduler,
            project=scheduler_project,
            workspace=workspace,
            dtstart=timezone.now() - timedelta(days=1),
            rrule="FREQ=WEEKLY",
            tzid="UTC",
            actor=user,
        )


def _make_run(binding, pod, user, **overrides):
    values = {
        "workspace": binding.workspace,
        "created_by": user,
        "pod": pod,
        "prompt": "",
        "scheduler_binding": binding,
        "status": "completed",
    }
    values.update(overrides)
    return AgentRun.objects.create(**values)


def _make_member(
    workspace,
    scheduler_project,
    *,
    username,
    workspace_role=15,
    project_role=None,
):
    user = User.objects.create(username=username, email=username)
    WorkspaceMember.objects.create(
        workspace=workspace,
        member=user,
        role=workspace_role,
        is_active=True,
    )
    if project_role is not None:
        ProjectMember.objects.create(
            project=scheduler_project,
            workspace=workspace,
            member=user,
            role=project_role,
            is_active=True,
        )
    return user


def _get_binding_runs(workspace, scheduler_project, binding, user, query=None):
    request = APIRequestFactory().get(
        f"/api/workspaces/{workspace.slug}/projects/{scheduler_project.id}/scheduler-bindings/{binding.id}/runs/",
        query or {},
    )
    force_authenticate(request, user=user)
    return ProjectSchedulerBindingRunsEndpoint.as_view()(
        request,
        slug=workspace.slug,
        project_id=str(scheduler_project.id),
        binding_id=str(binding.id),
    )


def _get_binding_detail(workspace, scheduler_project, binding, user):
    request = APIRequestFactory().get(
        f"/api/workspaces/{workspace.slug}/projects/{scheduler_project.id}/scheduler-bindings/{binding.id}/"
    )
    force_authenticate(request, user=user)
    return ProjectSchedulerBindingDetailEndpoint.as_view()(
        request,
        slug=workspace.slug,
        project_id=str(scheduler_project.id),
        binding_id=str(binding.id),
    )


def _get_binding_list(workspace, scheduler_project, user):
    request = APIRequestFactory().get(
        f"/api/workspaces/{workspace.slug}/projects/{scheduler_project.id}/scheduler-bindings/"
    )
    force_authenticate(request, user=user)
    return ProjectSchedulerBindingListEndpoint.as_view()(
        request,
        slug=workspace.slug,
        project_id=str(scheduler_project.id),
    )


def _get_run_detail(run, user):
    request = APIRequestFactory().get(f"/api/runners/runs/{run.id}/")
    force_authenticate(request, user=user)
    return AgentRunDetailEndpoint.as_view()(request, run_id=str(run.id))


@pytest.mark.unit
def test_binding_runs_are_isolated_and_newest_first(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    other = _make_binding(scheduler_project, workspace, create_user)
    first = _make_run(binding, default_pod, create_user)
    second = _make_run(binding, default_pod, create_user)
    _make_run(other, default_pod, create_user)

    response = _get_binding_runs(workspace, scheduler_project, binding, create_user)

    assert response.status_code == 200
    assert [row["id"] for row in response.data["results"]] == [
        str(second.id),
        str(first.id),
    ]
    assert response.data["total_count"] == 2
    assert response.data["results"][0]["scheduler_binding_detail"] == {
        "id": str(binding.id),
        "scheduler_name": "History Audit",
        "scheduler_slug": "history-audit",
        "project": str(scheduler_project.id),
    }


@pytest.mark.unit
def test_binding_runs_empty_history_envelope(workspace, scheduler_project, binding, create_user):
    response = _get_binding_runs(workspace, scheduler_project, binding, create_user)

    assert response.status_code == 200
    assert response.data == {
        "results": [],
        "count": 0,
        "total_count": 0,
        "total_pages": 1,
        "page": 1,
        "per_page": 30,
    }


@pytest.mark.unit
def test_binding_runs_pagination(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    for _ in range(3):
        _make_run(binding, default_pod, create_user)

    response = _get_binding_runs(
        workspace,
        scheduler_project,
        binding,
        create_user,
        {"page": "2", "per_page": "2"},
    )

    assert response.status_code == 200
    assert response.data["page"] == 2
    assert response.data["per_page"] == 2
    assert response.data["total_count"] == 3
    assert response.data["total_pages"] == 2
    assert response.data["count"] == 1


@pytest.mark.unit
@pytest.mark.parametrize(
    ("role_name", "workspace_role", "project_role"),
    [
        ("workspace-admin", 20, 15),
        ("project-admin", 15, 20),
        ("ordinary-member", 15, 15),
        ("project-guest", 5, 5),
    ],
)
def test_binding_runs_readable_by_supported_project_roles(
    role_name,
    workspace_role,
    project_role,
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    member = _make_member(
        workspace,
        scheduler_project,
        username=f"{role_name}@example.com",
        workspace_role=workspace_role,
        project_role=project_role,
    )
    _make_run(binding, default_pod, create_user)

    response = _get_binding_runs(workspace, scheduler_project, binding, member)

    assert response.status_code == 200
    assert response.data["total_count"] == 1


@pytest.mark.unit
def test_binding_runs_forbidden_outside_project(workspace, scheduler_project, binding):
    outsider = _make_member(
        workspace,
        scheduler_project,
        username="outsider@example.com",
        project_role=None,
    )

    response = _get_binding_runs(workspace, scheduler_project, binding, outsider)

    assert response.status_code == 403


@pytest.mark.unit
def test_binding_runs_wrong_project_is_not_found(workspace, scheduler_project, binding, create_user):
    with impersonate(create_user):
        other_project = Project.objects.create(
            name="Other",
            identifier="OTHER",
            workspace=workspace,
            created_by=create_user,
        )
    ProjectMember.objects.create(
        project=other_project,
        workspace=workspace,
        member=create_user,
        role=20,
    )

    response = _get_binding_runs(workspace, other_project, binding, create_user)

    assert response.status_code == 404


@pytest.mark.unit
def test_binding_runs_keep_private_runner_history_private(
    workspace,
    scheduler_project,
    binding,
    default_pod,
):
    owner = _make_member(
        workspace,
        scheduler_project,
        username="runner-owner@example.com",
        project_role=15,
    )
    viewer = _make_member(
        workspace,
        scheduler_project,
        username="history-viewer@example.com",
        project_role=15,
    )
    machine = DevMachine.objects.create(owner=owner, host_label="owner.local", label="Owner")
    runner = Runner.objects.create(
        owner=owner,
        workspace=workspace,
        pod=default_pod,
        name="private-runner",
        dev_machine=machine,
    )
    private_run = _make_run(
        binding,
        default_pod,
        owner,
        runner=runner,
        status="failed",
        ended_at=timezone.now(),
        error="private runner secret",
    )
    visible = _make_run(binding, default_pod, owner)
    binding.last_run = private_run
    binding.last_error = "private runner secret"
    binding.save(update_fields=["last_run", "last_error"])

    response = _get_binding_runs(workspace, scheduler_project, binding, viewer)
    detail_response = _get_binding_detail(workspace, scheduler_project, binding, viewer)
    list_response = _get_binding_list(workspace, scheduler_project, viewer)
    owner_response = _get_binding_runs(workspace, scheduler_project, binding, owner)
    owner_detail_response = _get_binding_detail(workspace, scheduler_project, binding, owner)

    assert [row["id"] for row in response.data["results"]] == [str(visible.id)]
    assert detail_response.status_code == 200
    assert detail_response.data["run_count"] == 1
    assert detail_response.data["last_run"] is None
    assert detail_response.data["last_run_status"] is None
    assert detail_response.data["last_run_ended_at"] is None
    assert detail_response.data["last_error"] == ""
    list_summary = list_response.data[0]
    assert list_summary["last_run"] is None
    assert list_summary["last_run_status"] is None
    assert list_summary["last_run_ended_at"] is None
    assert list_summary["last_error"] == ""
    assert owner_response.data["total_count"] == 2
    assert owner_detail_response.data["last_run"] == str(private_run.id)
    assert owner_detail_response.data["last_run_status"] == "failed"
    assert owner_detail_response.data["last_error"] == "private runner secret"


@pytest.mark.unit
def test_binding_summary_keeps_runnerless_last_run_project_visible(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    viewer = _make_member(
        workspace,
        scheduler_project,
        username="runnerless-viewer@example.com",
        project_role=15,
    )
    run = _make_run(
        binding,
        default_pod,
        create_user,
        status="failed",
        ended_at=timezone.now(),
        error="shared scheduler failure",
    )
    binding.last_run = run
    binding.last_error = "shared scheduler failure"
    binding.save(update_fields=["last_run", "last_error"])

    detail_response = _get_binding_detail(workspace, scheduler_project, binding, viewer)
    list_response = _get_binding_list(workspace, scheduler_project, viewer)

    assert detail_response.data["last_run"] == str(run.id)
    assert detail_response.data["last_run_status"] == "failed"
    assert detail_response.data["last_run_ended_at"] is not None
    assert detail_response.data["last_error"] == "shared scheduler failure"
    assert list_response.data[0]["last_run"] == str(run.id)
    assert list_response.data[0]["last_error"] == "shared scheduler failure"


@pytest.mark.unit
def test_binding_summary_keeps_runless_scheduler_error_project_visible(
    workspace,
    scheduler_project,
    binding,
):
    viewer = _make_member(
        workspace,
        scheduler_project,
        username="runless-error-viewer@example.com",
        project_role=15,
    )
    binding.last_error = "dispatch failed: no default pod"
    binding.save(update_fields=["last_error"])

    detail_response = _get_binding_detail(workspace, scheduler_project, binding, viewer)

    assert detail_response.data["last_run"] is None
    assert detail_response.data["last_error"] == "dispatch failed: no default pod"


@pytest.mark.unit
def test_private_scheduler_run_visible_only_to_runner_owner_or_run_creator(
    workspace,
    scheduler_project,
    binding,
    default_pod,
):
    owner = _make_member(
        workspace,
        scheduler_project,
        username="matrix-owner@example.com",
        project_role=15,
    )
    creator = _make_member(
        workspace,
        scheduler_project,
        username="matrix-creator@example.com",
        project_role=15,
    )
    member = _make_member(
        workspace,
        scheduler_project,
        username="matrix-member@example.com",
        project_role=15,
    )
    workspace_admin = _make_member(
        workspace,
        scheduler_project,
        username="matrix-admin@example.com",
        workspace_role=20,
        project_role=15,
    )
    machine = DevMachine.objects.create(owner=owner, host_label="matrix-owner.local", label="Matrix owner")
    runner = Runner.objects.create(
        owner=owner,
        workspace=workspace,
        pod=default_pod,
        name="matrix-private-runner",
        dev_machine=machine,
    )
    run = _make_run(binding, default_pod, creator, runner=runner)

    assert _get_binding_runs(workspace, scheduler_project, binding, owner).data["total_count"] == 1
    assert _get_binding_runs(workspace, scheduler_project, binding, creator).data["total_count"] == 1
    assert _get_binding_runs(workspace, scheduler_project, binding, member).data["total_count"] == 0
    assert _get_binding_runs(workspace, scheduler_project, binding, workspace_admin).data["total_count"] == 0
    assert _get_run_detail(run, owner).status_code == 200
    assert _get_run_detail(run, creator).status_code == 200
    assert _get_run_detail(run, member).status_code == 404
    assert _get_run_detail(run, workspace_admin).status_code == 404


@pytest.mark.unit
def test_binding_detail_exposes_prompt_and_run_count(
    workspace,
    scheduler_project,
    scheduler,
    binding,
    default_pod,
    create_user,
):
    binding.extra_context = "Focus on the API."
    binding.save(update_fields=["extra_context"])
    _make_run(binding, default_pod, create_user)
    request = APIRequestFactory().get(
        f"/api/workspaces/{workspace.slug}/projects/{scheduler_project.id}/scheduler-bindings/{binding.id}/"
    )
    force_authenticate(request, user=create_user)

    response = ProjectSchedulerBindingDetailEndpoint.as_view()(
        request,
        slug=workspace.slug,
        project_id=str(scheduler_project.id),
        binding_id=str(binding.id),
    )

    assert response.status_code == 200
    assert response.data["resolved_prompt"].startswith("Audit the project.\n\nFocus on the API.")
    assert response.data["run_count"] == 1
    assert response.data["scheduler_source"] == scheduler.source
    assert response.data["scheduler_is_enabled"] is True


@pytest.mark.unit
def test_scheduler_run_detail_visible_to_ordinary_project_member(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    member = _make_member(
        workspace,
        scheduler_project,
        username="member@example.com",
        project_role=15,
    )
    run = _make_run(binding, default_pod, create_user)

    response = _get_run_detail(run, member)

    assert response.status_code == 200
    assert response.data["scheduler_binding_detail"]["scheduler_slug"] == ("history-audit")


@pytest.mark.unit
def test_scheduler_run_detail_hidden_from_workspace_member_outside_project(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    outsider = _make_member(
        workspace,
        scheduler_project,
        username="run-outsider@example.com",
        project_role=None,
    )
    run = _make_run(binding, default_pod, create_user)

    response = _get_run_detail(run, outsider)

    assert response.status_code == 404


@pytest.mark.unit
@pytest.mark.parametrize("delete_issue", [False, True])
def test_scheduler_run_detail_survives_invisible_or_deleted_work_item(
    delete_issue,
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    member = _make_member(
        workspace,
        scheduler_project,
        username=f"legacy-member-{delete_issue}@example.com",
        project_role=15,
    )
    with impersonate(create_user):
        hidden_project = Project.objects.create(
            name="Hidden",
            identifier=f"HID{int(delete_issue)}",
            workspace=workspace,
            created_by=create_user,
        )
        issue = Issue.objects.create(
            name="Legacy linked item",
            project=hidden_project,
            workspace=workspace,
            created_by=create_user,
        )
    run = _make_run(binding, default_pod, create_user, work_item=issue)
    if delete_issue:
        issue.delete()

    response = _get_run_detail(run, member)

    assert response.status_code == 200
    assert response.data["id"] == str(run.id)


@pytest.mark.unit
def test_global_runs_filter_and_malformed_uuid(
    workspace,
    scheduler_project,
    binding,
    default_pod,
    create_user,
):
    other = _make_binding(scheduler_project, workspace, create_user, "weekly-audit")
    expected = _make_run(binding, default_pod, create_user)
    _make_run(other, default_pod, create_user)
    factory = APIRequestFactory()

    request = factory.get("/api/runners/runs/", {"scheduler_binding": str(binding.id)})
    force_authenticate(request, user=create_user)
    response = AgentRunListEndpoint.as_view()(request)

    invalid_request = factory.get("/api/runners/runs/", {"scheduler_binding": "not-a-uuid"})
    force_authenticate(invalid_request, user=create_user)
    invalid_response = AgentRunListEndpoint.as_view()(invalid_request)

    assert response.status_code == 200
    assert [row["id"] for row in response.data["results"]] == [str(expected.id)]
    assert invalid_response.status_code == 400
