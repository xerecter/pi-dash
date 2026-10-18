"""Machine-scoped project snapshots and automatic runner binding contracts."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from threading import Barrier
from uuid import uuid4
from unittest.mock import patch

import pytest
from django.db import close_old_connections, connection, transaction
from django.test import override_settings
from django.urls import reverse
from django.utils import timezone
from rest_framework.test import APIClient

from karma_team.db.models import Project, ProjectMember, User, Workspace, WorkspaceMember
from karma_team.runner.models import (
    DevMachine,
    MachineToken,
    Pod,
    ProjectDeletionTombstone,
    Runner,
    RunnerBindingSource,
    RunnerProvisioning,
)
from karma_team.runner.services import tokens
from karma_team.runner.services.project_visibility import (
    projects_visible_to_machine_token,
)
from karma_team.runner.views.projects import _machine_project_snapshot


@pytest.fixture(autouse=True)
def _default_project_membership(workspace, project, create_user):
    ProjectMember.objects.get_or_create(
        workspace=workspace, project=project, member=create_user, defaults={"role": 20}
    )


def _machine_token(
    *,
    create_user,
    workspace,
    host_label="sync-host",
    dev_machine_id=None,
    provisioning=RunnerProvisioning.MANUAL,
):
    machine = DevMachine.objects.create(
        id=dev_machine_id or uuid4(),
        owner=create_user,
        host_label=host_label,
        label=host_label,
        provisioning=provisioning,
    )
    minted = tokens.mint_machine_token()
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=host_label,
        token_hash=minted.hashed,
        token_fingerprint=minted.fingerprint,
        label=f"machine: {host_label}",
        is_service=True,
    )
    return machine, minted.raw


def _bearer(client, raw_token):
    client.credentials(HTTP_AUTHORIZATION=f"Bearer {raw_token}")
    return client


def _snapshot_url(machine):
    return reverse(
        "runner:machine-project-bindings",
        kwargs={"dev_machine_id": machine.id},
    )


def _ensure_url(machine):
    return reverse(
        "runner:machine-project-binding-ensure",
        kwargs={"dev_machine_id": machine.id},
    )


def _runner_delete_url(runner):
    return reverse("runner:runner-self-revoke", kwargs={"runner_id": runner.id})


def _api_runner_delete_url(runner):
    return reverse("api-runner-delete", kwargs={"runner_id": runner.id})


@pytest.mark.unit
def test_existing_machine_discovers_and_binds_project_joined_in_another_workspace(
    db, api_client, create_user, workspace, project
):
    machine, primary_token = _machine_token(create_user=create_user, workspace=workspace)
    other_user = User.objects.create(
        email=f"project-owner-{uuid4().hex[:8]}@example.com", username=f"owner-{uuid4().hex[:8]}"
    )
    shared = Workspace.objects.create(name="Shared", slug=f"shared-{uuid4().hex[:8]}", owner=other_user)
    WorkspaceMember.objects.create(workspace=shared, member=other_user, role=20)
    membership = WorkspaceMember.objects.create(workspace=shared, member=create_user, role=15)
    joined_project = Project.objects.create(
        name="Someone else's project",
        identifier=project.identifier,
        workspace=shared,
        created_by=other_user,
        network=0,
    )
    api_client.credentials(HTTP_X_API_KEY=primary_token)
    inventory = api_client.get("/api/v1/auth/workspaces/")
    assert inventory.status_code == 200, inventory.data
    assert {workspace.slug, shared.slug} <= {w["slug"] for w in inventory.data["workspaces"]}
    exchange = api_client.post(
        "/api/v1/auth/machine-token/",
        {
            "workspace_slug": shared.slug,
            "dev_machine_id": str(machine.id),
            "host_label": machine.host_label,
        },
        format="json",
    )
    assert exchange.status_code == 201, exchange.data
    shared_token = exchange.data["machine_token"]
    api_client.credentials(HTTP_AUTHORIZATION=f"Bearer {shared_token}")
    assert api_client.get(_snapshot_url(machine)).data["projects"] == []
    assert (
        api_client.post(_ensure_url(machine), {"project_id": str(joined_project.id)}, format="json").status_code == 404
    )

    project_member = ProjectMember.objects.create(
        workspace=shared,
        project=joined_project,
        member=create_user,
        role=15,
    )
    snapshot = api_client.get(_snapshot_url(machine))
    assert snapshot.status_code == 200, snapshot.data
    assert [p["id"] for p in snapshot.data["projects"]] == [str(joined_project.id)]
    payload = {"project_id": str(joined_project.id)}
    first = api_client.post(_ensure_url(machine), payload, format="json")
    again = api_client.post(_ensure_url(machine), payload, format="json")
    assert first.status_code == 201, first.data
    assert again.status_code == 200, again.data
    assert first.data["runner_id"] == again.data["runner_id"]
    runner = Runner.objects.get(pk=first.data["runner_id"])
    assert runner.owner_id == create_user.id
    assert runner.pod.project_id == joined_project.id
    assert runner.workspace_id == shared.id
    assert runner.binding_source == RunnerBindingSource.AUTO_PROJECT_SYNC

    primary = _bearer(api_client, primary_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )
    assert primary.status_code == 201, primary.data
    assert primary.data["runner_name"] != first.data["runner_name"]
    assert _bearer(api_client, primary_token).post(_ensure_url(machine), payload, format="json").status_code == 404
    project_member.is_active = False
    project_member.save(update_fields=["is_active"])
    assert _bearer(api_client, shared_token).get(_snapshot_url(machine)).data["projects"] == []
    membership.is_active = False
    membership.save(update_fields=["is_active"])
    assert _bearer(api_client, shared_token).get(_snapshot_url(machine)).status_code in (401, 403)


@pytest.mark.unit
def test_machine_snapshot_is_complete_and_scoped_to_token_workspace_and_machine(
    db, api_client, create_user, workspace, project
):
    second_project = Project.objects.create(
        name="Second Project",
        identifier="SECOND",
        workspace=workspace,
        created_by=create_user,
    )
    ProjectMember.objects.create(
        project=second_project, workspace=workspace, member=create_user, role=20
    )
    deleted_project = Project.objects.create(
        name="Deleted Project",
        identifier="DELETED",
        workspace=workspace,
        created_by=create_user,
    )
    deleted_project_pod = Pod.default_for_project_id(deleted_project.id)

    other_user = type(create_user).objects.create(
        username=f"other-{uuid4().hex[:8]}",
        email=f"other-{uuid4().hex[:8]}@example.com",
        first_name="Other",
        last_name="User",
    )
    other_workspace = Workspace.objects.create(
        name="Other Workspace",
        slug=f"other-{uuid4().hex[:8]}",
        owner=other_user,
    )
    WorkspaceMember.objects.create(
        workspace=other_workspace,
        member=other_user,
        role=20,
    )
    other_project = Project.objects.create(
        name="Other Project",
        identifier="OTHER",
        workspace=other_workspace,
        created_by=other_user,
    )

    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    default_pod = Pod.default_for_project_id(project.id)
    runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=default_pod,
        name="existing-runner",
    )
    deleted_project_runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=deleted_project_pod,
        name="deleted-project-runner",
    )
    # Avoid Project.delete()'s Celery cascade in this focused API test. The
    # snapshot must independently exclude both the tombstoned project and an
    # otherwise-active runner still pointing at one of its pods.
    deleted_project.deleted_at = timezone.now()
    deleted_project.save(update_fields=["deleted_at"])
    ProjectDeletionTombstone.objects.create(
        project_id=deleted_project.id,
        workspace_id=workspace.id,
    )
    # A runner on another machine must not leak into this machine's bindings.
    other_machine, _ = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="other-machine",
    )
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=other_machine,
        pod=Pod.default_for_project_id(second_project.id),
        name="other-machine-runner",
    )

    response = _bearer(api_client, raw_token).get(_snapshot_url(machine))

    assert response.status_code == 200, response.data
    assert response.data["complete"] is True
    assert response.data["workspace_id"] == str(workspace.id)
    assert response.data["workspace_slug"] == workspace.slug
    project_ids = {row["id"] for row in response.data["projects"]}
    assert project_ids == {str(project.id), str(second_project.id)}
    assert str(deleted_project.id) not in project_ids
    assert str(other_project.id) not in project_ids
    assert response.data["deleted_project_ids"] == [str(deleted_project.id)]
    assert str(other_project.id) not in response.data["deleted_project_ids"]
    assert response.data["bindings"] == [
        {
            "runner_id": str(runner.id),
            "runner_name": runner.name,
            "project_id": str(project.id),
            "project_identifier": project.identifier,
            "pod_id": str(default_pod.id),
            "pod_slug": default_pod.name,
            "binding_source": RunnerBindingSource.MANUAL,
        }
    ]
    assert str(deleted_project_runner.id) not in {row["runner_id"] for row in response.data["bindings"]}


@pytest.mark.unit
def test_ensure_binding_is_idempotent_for_repeated_requests(db, api_client, create_user, workspace, project):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    url = _ensure_url(machine)
    payload = {"project_id": str(project.id)}

    responses = [_bearer(api_client, raw_token).post(url, payload, format="json") for _ in range(10)]

    assert responses[0].status_code == 201, responses[0].data
    assert all(response.status_code in {200, 201} for response in responses)
    runner_ids = {response.data["runner_id"] for response in responses}
    assert len(runner_ids) == 1
    runner = Runner.objects.get(pk=next(iter(runner_ids)))
    assert runner.binding_source == RunnerBindingSource.AUTO_PROJECT_SYNC
    assert runner.dev_machine_id == machine.id
    assert runner.project_id == project.id
    assert (
        Runner.objects.filter(
            dev_machine=machine,
            pod=runner.pod,
            binding_source=RunnerBindingSource.AUTO_PROJECT_SYNC,
            revoked_at__isnull=True,
        ).count()
        == 1
    )


@pytest.mark.unit
def test_ensure_reuses_existing_manual_runner_for_same_machine_and_project(
    db, api_client, create_user, workspace, project
):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    pod = Pod.default_for_project_id(project.id)
    manual = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=pod,
        name="manual-runner",
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 200, response.data
    assert response.data["runner_id"] == str(manual.id)
    assert Runner.objects.filter(dev_machine=machine, pod=pod).count() == 1
    manual.refresh_from_db()
    assert manual.binding_source == RunnerBindingSource.MANUAL


@pytest.mark.unit
def test_ensure_binding_inherits_desktop_machine_provisioning(db, api_client, create_user, workspace, project):
    machine, raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 201, response.data
    runner = Runner.objects.get(pk=response.data["runner_id"])
    assert runner.provisioning == RunnerProvisioning.DESKTOP_BUNDLED
    assert runner.binding_source == RunnerBindingSource.AUTO_PROJECT_SYNC


@pytest.mark.unit
@override_settings(MANAGED_RUNNER_MAX_PER_USER_PROJECT=1)
def test_ensure_binding_applies_desktop_managed_runner_cap(db, api_client, create_user, workspace, project):
    existing_machine = DevMachine.objects.create(
        owner=create_user,
        host_label="existing-desktop",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    pod = Pod.default_for_project_id(project.id)
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=existing_machine,
        pod=pod,
        name="existing-desktop-runner",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    machine, raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="second-desktop",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 409, response.data
    assert response.data["error"] == "managed_runner_limit"
    assert not Runner.objects.filter(dev_machine=machine, pod=pod).exists()


@pytest.mark.unit
@override_settings(MANAGED_RUNNER_MAX_PER_USER_PROJECT=1)
def test_ensure_binding_applies_desktop_cap_across_project_pods(db, api_client, create_user, workspace, project):
    non_default_pod = Pod.objects.create(
        workspace=workspace,
        project=project,
        name="desktop-non-default",
        created_by=create_user,
    )
    existing_machine = DevMachine.objects.create(
        owner=create_user,
        host_label="desktop-on-other-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=existing_machine,
        pod=non_default_pod,
        name="desktop-on-other-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    machine, raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="desktop-default-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 409, response.data
    assert response.data["error"] == "managed_runner_limit"
    assert not Runner.objects.filter(dev_machine=machine).exists()


@pytest.mark.unit
@override_settings(MANAGED_RUNNER_MAX_PER_USER_PROJECT=1)
def test_ensure_binding_ignores_desktop_runner_on_deleted_pod_for_cap(db, api_client, create_user, workspace, project):
    deleted_pod = Pod.objects.create(
        workspace=workspace,
        project=project,
        name="deleted-desktop-pod",
        created_by=create_user,
    )
    stale_machine = DevMachine.objects.create(
        owner=create_user,
        host_label="desktop-on-deleted-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=stale_machine,
        pod=deleted_pod,
        name="desktop-on-deleted-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    deleted_pod.deleted_at = timezone.now()
    deleted_pod.save(update_fields=["deleted_at"])
    machine, raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="desktop-active-pod",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 201, response.data
    runner = Runner.objects.get(pk=response.data["runner_id"])
    assert runner.pod_id == Pod.default_for_project_id(project.id).id


@pytest.mark.unit
@override_settings(MANAGED_RUNNER_MAX_PER_USER_PROJECT=1)
def test_ensure_binding_keeps_manual_machine_behavior_when_desktop_cap_is_full(
    db, api_client, create_user, workspace, project
):
    desktop_machine = DevMachine.objects.create(
        owner=create_user,
        host_label="desktop-at-cap",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    pod = Pod.default_for_project_id(project.id)
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=desktop_machine,
        pod=pod,
        name="desktop-at-cap-runner",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    manual_machine, raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="manual-machine",
    )

    response = _bearer(api_client, raw_token).post(
        _ensure_url(manual_machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert response.status_code == 201, response.data
    runner = Runner.objects.get(pk=response.data["runner_id"])
    assert runner.provisioning == RunnerProvisioning.MANUAL


@pytest.mark.unit
def test_machine_snapshot_batches_default_pod_lookup(db, django_assert_num_queries, create_user, workspace, project):
    for index in range(5):
        added = Project.objects.create(
            name=f"Snapshot project {index}",
            identifier=f"SNAP{index}",
            workspace=workspace,
            created_by=create_user,
        )
        ProjectMember.objects.create(project=added, workspace=workspace, member=create_user, role=20)
    machine, _ = _machine_token(create_user=create_user, workspace=workspace)
    token = MachineToken.objects.select_related("workspace").get(dev_machine=machine)

    with django_assert_num_queries(4):
        snapshot = _machine_project_snapshot(machine, token)

    assert len(snapshot["projects"]) == 6


@pytest.mark.unit
def test_machine_project_visibility_requires_active_project_membership(db, api_client, create_user, workspace, project):
    public_project = Project.objects.create(
        name="Public unassigned",
        identifier="PUBLIC",
        workspace=workspace,
        created_by=create_user,
        network=2,
    )
    assigned_secret = Project.objects.create(
        name="Assigned secret",
        identifier="ASSIGNED",
        workspace=workspace,
        created_by=create_user,
        network=0,
    )
    inactive_secret = Project.objects.create(
        name="Inactive secret membership",
        identifier="INACTIVE",
        workspace=workspace,
        created_by=create_user,
        network=0,
    )
    hidden_secret = Project.objects.create(
        name="Hidden secret",
        identifier="HIDDEN",
        workspace=workspace,
        created_by=create_user,
        network=0,
    )
    ProjectMember.objects.create(
        project=assigned_secret,
        workspace=workspace,
        member=create_user,
        role=15,
        is_active=True,
    )
    ProjectMember.objects.create(
        project=inactive_secret,
        workspace=workspace,
        member=create_user,
        role=15,
        is_active=False,
    )
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    runners = {}
    for target in (public_project, assigned_secret, inactive_secret, hidden_secret):
        runners[target.identifier] = Runner.objects.create(
            owner=create_user,
            workspace=workspace,
            dev_machine=machine,
            pod=Pod.default_for_project_id(target.id),
            name=f"visibility-{target.identifier.lower()}",
        )

    def snapshot_ids():
        response = _bearer(api_client, raw_token).get(_snapshot_url(machine))
        assert response.status_code == 200, response.data
        assert response.data["deleted_project_ids"] == []
        return (
            {row["id"] for row in response.data["projects"]},
            {row["runner_id"] for row in response.data["bindings"]},
        )

    admin_projects, admin_bindings = snapshot_ids()
    assert admin_projects == {str(project.id), str(assigned_secret.id)}
    assert admin_bindings == {str(runners["ASSIGNED"].id)}

    WorkspaceMember.objects.filter(workspace=workspace, member=create_user).update(role=15)
    member_projects, member_bindings = snapshot_ids()
    assert str(project.id) in member_projects
    assert str(public_project.id) not in member_projects
    assert str(assigned_secret.id) in member_projects
    assert str(inactive_secret.id) not in member_projects
    assert str(hidden_secret.id) not in member_projects
    assert member_bindings == {str(runners["ASSIGNED"].id)}

    WorkspaceMember.objects.filter(workspace=workspace, member=create_user).update(role=5)
    guest_projects, guest_bindings = snapshot_ids()
    assert guest_projects == {str(project.id), str(assigned_secret.id)}
    assert guest_bindings == {str(runners["ASSIGNED"].id)}


@pytest.mark.unit
def test_machine_ensure_uses_the_same_project_visibility_as_snapshot(db, api_client, create_user, workspace, project):
    public_project = Project.objects.create(
        name="Unassigned public project",
        identifier="ENSPUBLIC",
        workspace=workspace,
        created_by=create_user,
        network=2,
    )
    assigned_secret = Project.objects.create(
        name="Ensure assigned secret",
        identifier="ENSASSIGN",
        workspace=workspace,
        created_by=create_user,
        network=0,
    )
    hidden_secret = Project.objects.create(
        name="Ensure hidden secret",
        identifier="ENSHIDDEN",
        workspace=workspace,
        created_by=create_user,
        network=0,
    )
    ProjectMember.objects.create(
        project=assigned_secret,
        workspace=workspace,
        member=create_user,
        role=5,
        is_active=True,
    )
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)

    # Neither workspace admins nor public-project viewers can run work items
    # without an active ProjectMember row.
    admin_hidden = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(hidden_secret.id)}, format="json"
    )
    assert admin_hidden.status_code == 404, admin_hidden.data

    WorkspaceMember.objects.filter(workspace=workspace, member=create_user).update(role=15)
    member_public = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(project.id)}, format="json"
    )
    assert member_public.status_code == 201, member_public.data
    unassigned_public = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(public_project.id)}, format="json"
    )
    assert unassigned_public.status_code == 404, unassigned_public.data
    member_hidden = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(hidden_secret.id)}, format="json"
    )
    assert member_hidden.status_code == 404, member_hidden.data
    assert member_hidden.data["error"] == "project_not_found"

    WorkspaceMember.objects.filter(workspace=workspace, member=create_user).update(role=5)
    guest_assigned = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(assigned_secret.id)}, format="json"
    )
    assert guest_assigned.status_code == 201, guest_assigned.data
    guest_public = _bearer(api_client, raw_token).post(
        _ensure_url(machine), {"project_id": str(public_project.id)}, format="json"
    )
    assert guest_public.status_code == 404, guest_public.data
    assert guest_public.data["error"] == "project_not_found"


@pytest.mark.unit
def test_machine_visibility_query_remains_lockable_without_distinct(db, create_user, workspace):
    machine, _ = _machine_token(create_user=create_user, workspace=workspace)
    token = MachineToken.objects.get(dev_machine=machine)

    queryset = projects_visible_to_machine_token(Project.objects.select_for_update(of=("self",)), token)

    assert queryset.query.select_for_update is True
    assert queryset.query.select_for_update_of == ("self",)
    assert queryset.query.distinct is False


@pytest.mark.unit
def test_machine_snapshot_excludes_revoked_runner_and_runner_on_deleted_pod(
    db, api_client, create_user, workspace, project
):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    default_pod = Pod.default_for_project_id(project.id)
    active = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=default_pod,
        name="active-binding",
    )
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=default_pod,
        name="revoked-binding",
        revoked_at=timezone.now(),
    )
    deleted_pod = Pod.objects.create(
        workspace=workspace,
        project=project,
        name="deleted-pod",
        created_by=create_user,
    )
    Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=deleted_pod,
        name="deleted-pod-binding",
    )
    deleted_pod.deleted_at = timezone.now()
    deleted_pod.save(update_fields=["deleted_at"])

    response = _bearer(api_client, raw_token).get(_snapshot_url(machine))

    assert response.status_code == 200, response.data
    assert [row["runner_id"] for row in response.data["bindings"]] == [str(active.id)]


@pytest.mark.unit
def test_machine_snapshot_and_ensure_reject_non_machine_credentials(
    db, session_client, api_token, create_user, workspace, project
):
    machine = DevMachine.objects.create(owner=create_user, host_label="machine-auth-only")
    payload = {"project_id": str(project.id)}

    api_key_client = APIClient()
    api_key_client.credentials(HTTP_X_API_KEY=api_token.token)
    api_snapshot = api_key_client.get(_snapshot_url(machine))
    api_ensure = api_key_client.post(_ensure_url(machine), payload, format="json")
    session_snapshot = session_client.get(_snapshot_url(machine))
    session_ensure = session_client.post(_ensure_url(machine), payload, format="json")

    assert api_snapshot.status_code == 403
    assert api_ensure.status_code == 403
    assert session_snapshot.status_code == 403
    assert session_ensure.status_code == 403
    assert Runner.objects.count() == 0


@pytest.mark.unit
def test_machine_snapshot_and_ensure_reject_a_different_machine_url(db, api_client, create_user, workspace, project):
    _, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    other_machine = DevMachine.objects.create(owner=create_user, host_label="different-machine")
    payload = {"project_id": str(project.id)}

    snapshot = _bearer(api_client, raw_token).get(_snapshot_url(other_machine))
    ensure = _bearer(api_client, raw_token).post(_ensure_url(other_machine), payload, format="json")

    assert snapshot.status_code == 403
    assert ensure.status_code == 403
    assert Runner.objects.count() == 0


@pytest.mark.unit
def test_same_machine_tokens_are_strictly_scoped_to_their_workspace(db, api_client, create_user, workspace, project):
    machine, first_raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
    )
    other_workspace = Workspace.objects.create(
        name="Other workspace on same machine",
        slug=f"same-machine-{uuid4().hex[:8]}",
        owner=create_user,
    )
    WorkspaceMember.objects.create(
        workspace=other_workspace,
        member=create_user,
        role=20,
    )
    other_project = Project.objects.create(
        name="Other workspace project",
        identifier="OTHERWS",
        workspace=other_workspace,
        created_by=create_user,
    )
    ProjectMember.objects.create(
        workspace=other_workspace, project=other_project, member=create_user, role=20
    )
    second_minted = tokens.mint_machine_token()
    MachineToken.objects.create(
        user=create_user,
        workspace=other_workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=second_minted.hashed,
        token_fingerprint=second_minted.fingerprint,
        label=f"machine: {machine.host_label}",
        is_service=True,
    )

    first_snapshot = _bearer(api_client, first_raw_token).get(_snapshot_url(machine))
    first_cross_workspace_ensure = _bearer(api_client, first_raw_token).post(
        _ensure_url(machine),
        {"project_id": str(other_project.id)},
        format="json",
    )
    second_snapshot = _bearer(api_client, second_minted.raw).get(_snapshot_url(machine))
    second_cross_workspace_ensure = _bearer(api_client, second_minted.raw).post(
        _ensure_url(machine),
        {"project_id": str(project.id)},
        format="json",
    )

    assert first_snapshot.status_code == 200, first_snapshot.data
    assert {row["id"] for row in first_snapshot.data["projects"]} == {str(project.id)}
    assert first_cross_workspace_ensure.status_code == 404
    assert first_cross_workspace_ensure.data["error"] == "project_not_found"
    assert second_snapshot.status_code == 200, second_snapshot.data
    assert {row["id"] for row in second_snapshot.data["projects"]} == {str(other_project.id)}
    assert second_cross_workspace_ensure.status_code == 404
    assert second_cross_workspace_ensure.data["error"] == "project_not_found"
    assert not Runner.objects.filter(dev_machine=machine).exists()


@pytest.mark.django_db(transaction=True)
def test_concurrent_ensure_requests_share_one_automatic_binding(create_user, workspace, project):
    if connection.vendor != "postgresql":
        pytest.skip("row-lock concurrency contract requires PostgreSQL")

    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    url = _ensure_url(machine)
    payload = {"project_id": str(project.id)}
    worker_count = 8
    barrier = Barrier(worker_count)

    def ensure_once():
        close_old_connections()
        client = APIClient()
        client.credentials(HTTP_AUTHORIZATION=f"Bearer {raw_token}")
        barrier.wait(timeout=10)
        response = client.post(url, payload, format="json")
        result = (response.status_code, dict(response.data))
        close_old_connections()
        return result

    with ThreadPoolExecutor(max_workers=worker_count) as executor:
        results = list(executor.map(lambda _: ensure_once(), range(worker_count)))

    statuses = [status_code for status_code, _ in results]
    assert statuses.count(201) == 1, results
    assert all(status_code in {200, 201} for status_code in statuses), results
    runner_ids = {body["runner_id"] for _, body in results}
    assert len(runner_ids) == 1
    assert (
        Runner.objects.filter(
            dev_machine=machine,
            pod__project=project,
            binding_source=RunnerBindingSource.AUTO_PROJECT_SYNC,
            revoked_at__isnull=True,
        ).count()
        == 1
    )


@pytest.mark.django_db(transaction=True)
def test_concurrent_machine_delete_and_ensure_never_orphans_a_runner(create_user, workspace, project):
    if connection.vendor != "postgresql":
        pytest.skip("row-lock concurrency contract requires PostgreSQL")

    from karma_team.runner.services.runner_delete import delete_dev_machine

    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    machine_id = machine.id
    barrier = Barrier(2)

    def delete_once():
        close_old_connections()
        target = DevMachine.objects.get(pk=machine_id)
        barrier.wait(timeout=10)
        delete_dev_machine(target, purge_local=True)
        close_old_connections()
        return "deleted"

    def ensure_once():
        close_old_connections()
        client = APIClient()
        client.credentials(HTTP_AUTHORIZATION=f"Bearer {raw_token}")
        barrier.wait(timeout=10)
        response = client.post(
            reverse(
                "runner:machine-project-binding-ensure",
                kwargs={"dev_machine_id": machine_id},
            ),
            {"project_id": str(project.id)},
            format="json",
        )
        result = response.status_code
        close_old_connections()
        return result

    with (
        patch("karma_team.runner.services.runner_delete.send_runner_remove"),
        patch("karma_team.runner.services.runner_delete.close_runner_session"),
        ThreadPoolExecutor(max_workers=2) as executor,
    ):
        delete_future = executor.submit(delete_once)
        ensure_future = executor.submit(ensure_once)
        assert delete_future.result(timeout=20) == "deleted"
        ensure_status = ensure_future.result(timeout=20)

    assert ensure_status in {201, 401, 403}
    assert not DevMachine.objects.filter(pk=machine_id).exists()
    assert not Runner.objects.filter(
        owner=create_user,
        workspace=workspace,
        pod__project=project,
    ).exists()
    assert not Runner.objects.filter(
        owner=create_user,
        workspace=workspace,
        dev_machine_id__isnull=True,
    ).exists()


@pytest.mark.django_db(transaction=True)
@override_settings(MANAGED_RUNNER_MAX_PER_USER_PROJECT=1)
def test_concurrent_desktop_ensure_requests_respect_managed_cap(create_user, workspace, project):
    if connection.vendor != "postgresql":
        pytest.skip("row-lock concurrency contract requires PostgreSQL")

    first_machine, first_raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="desktop-cap-first",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    second_machine, second_raw_token = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="desktop-cap-second",
        provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
    )
    participants = (
        (first_machine.id, first_raw_token),
        (second_machine.id, second_raw_token),
    )
    barrier = Barrier(len(participants))

    def ensure_once(participant):
        machine_id, raw_token = participant
        close_old_connections()
        client = APIClient()
        client.credentials(HTTP_AUTHORIZATION=f"Bearer {raw_token}")
        barrier.wait(timeout=10)
        response = client.post(
            reverse(
                "runner:machine-project-binding-ensure",
                kwargs={"dev_machine_id": machine_id},
            ),
            {"project_id": str(project.id)},
            format="json",
        )
        result = (response.status_code, dict(response.data))
        close_old_connections()
        return result

    with ThreadPoolExecutor(max_workers=len(participants)) as executor:
        results = list(executor.map(ensure_once, participants))

    statuses = sorted(status_code for status_code, _ in results)
    assert statuses == [201, 409], results
    rejected = next(body for status_code, body in results if status_code == 409)
    assert rejected["error"] == "managed_runner_limit"
    assert (
        Runner.objects.filter(
            workspace=workspace,
            pod__project=project,
            provisioning=RunnerProvisioning.DESKTOP_BUNDLED,
            revoked_at__isnull=True,
        ).count()
        == 1
    )


@pytest.mark.unit
def test_machine_snapshot_and_ensure_reject_machine_less_token(db, api_client, create_user, workspace, project):
    minted = tokens.mint_machine_token()
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=None,
        host_label="legacy-host",
        token_hash=minted.hashed,
        token_fingerprint=minted.fingerprint,
        label="machine: legacy-host",
        is_service=True,
    )
    machine = DevMachine.objects.create(owner=create_user, host_label="target")
    payload = {"project_id": str(project.id)}

    snapshot = _bearer(api_client, minted.raw).get(_snapshot_url(machine))
    ensure = _bearer(api_client, minted.raw).post(_ensure_url(machine), payload, format="json")

    assert snapshot.status_code == 403
    assert ensure.status_code == 403
    assert Runner.objects.count() == 0


@pytest.mark.unit
def test_machine_snapshot_rejects_revoked_machine_token(db, api_client, create_user, workspace, project):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    MachineToken.objects.filter(dev_machine=machine).update(revoked_at=timezone.now())

    response = _bearer(api_client, raw_token).get(_snapshot_url(machine))

    assert response.status_code == 401


@pytest.mark.unit
def test_machine_token_delete_is_limited_to_its_machine_and_purges_locally(
    db, api_client, create_user, workspace, project
):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    other_machine, _ = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="other-delete-machine",
    )
    pod = Pod.default_for_project_id(project.id)
    own_runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=pod,
        name="machine-delete-own",
    )
    other_runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=other_machine,
        pod=pod,
        name="machine-delete-other",
    )

    with patch("karma_team.runner.views.enrollment.delete_runner_svc") as delete_runner:
        response = _bearer(api_client, raw_token).delete(_runner_delete_url(own_runner))

    assert response.status_code == 204, response.data
    delete_runner.assert_called_once_with(own_runner, purge_local=True)
    assert Runner.objects.filter(pk=other_runner.id).exists()

    forbidden = _bearer(api_client, raw_token).delete(_runner_delete_url(other_runner))
    assert forbidden.status_code == 404
    assert Runner.objects.filter(pk=other_runner.id).exists()


@pytest.mark.unit
def test_machine_token_api_delete_is_limited_to_its_machine(db, api_client, create_user, workspace, project):
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    other_machine, _ = _machine_token(
        create_user=create_user,
        workspace=workspace,
        host_label="other-api-delete-machine",
    )
    runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=other_machine,
        pod=Pod.default_for_project_id(project.id),
        name="other-machine-api-delete",
    )

    api_client.credentials(HTTP_X_API_KEY=raw_token)
    response = api_client.delete(_api_runner_delete_url(runner))

    assert response.status_code == 404
    assert Runner.objects.filter(pk=runner.id).exists()


@pytest.mark.unit
def test_machine_token_delete_returns_404_when_runner_missing(db, api_client, create_user, workspace):
    _, raw_token = _machine_token(create_user=create_user, workspace=workspace)
    missing = _bearer(api_client, raw_token).delete(reverse("runner:runner-self-revoke", kwargs={"runner_id": uuid4()}))
    assert missing.status_code == 404


@pytest.mark.unit
def test_external_project_delete_removes_all_project_bindings(
    db,
    api_key_client,
    create_user,
    workspace,
    project,
    django_capture_on_commit_callbacks,
):
    target = Project.objects.create(
        name="External delete target",
        identifier="EXTDEL",
        workspace=workspace,
        created_by=create_user,
    )
    ProjectMember.objects.create(
        project=target,
        workspace=workspace,
        member=create_user,
        role=20,
        is_active=True,
    )
    machine, _ = _machine_token(create_user=create_user, workspace=workspace)
    pod = Pod.default_for_project_id(target.id)
    manual = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=pod,
        name="project-delete-manual",
    )
    automatic = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=pod,
        name="project-delete-automatic",
        binding_source=RunnerBindingSource.AUTO_PROJECT_SYNC,
    )
    other_project = Project.objects.create(
        name="Project delete survivor",
        identifier="SURVIVE",
        workspace=workspace,
        created_by=create_user,
    )
    survivor = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=Pod.default_for_project_id(other_project.id),
        name="project-delete-survivor",
    )
    enqueued = []

    with (
        patch(
            "karma_team.runner.services.runner_delete.send_to_machine",
            side_effect=lambda machine_id, message: enqueued.append((machine_id, message)),
        ),
        patch("karma_team.api.views.project.webhook_activity.delay"),
        patch("karma_team.db.mixins.soft_delete_related_objects.delay"),
        django_capture_on_commit_callbacks(execute=True),
    ):
        response = api_key_client.delete(f"/api/v1/workspaces/{workspace.slug}/projects/{target.id}/")

    assert response.status_code == 204, response.data
    assert {machine_id for machine_id, _message in enqueued} == {machine.id}
    assert {message["runner_id"] for _machine_id, message in enqueued} == {
        str(manual.id),
        str(automatic.id),
    }
    assert all(
        message["mid"] == f"project-delete:{target.id}:{message['runner_id']}" for _machine_id, message in enqueued
    )
    assert not Runner.objects.filter(pk__in=[manual.id, automatic.id]).exists()
    assert Runner.objects.filter(pk=survivor.id).exists()
    assert not Project.objects.filter(pk=target.id).exists()
    assert Project.all_objects.filter(
        pk=target.id,
        deleted_at__isnull=False,
    ).exists()
    assert (
        ProjectDeletionTombstone.objects.filter(
            project_id=target.id,
            workspace_id=workspace.id,
        ).count()
        == 1
    )

    # Retrying cleanup after the Project row was soft-deleted must reuse the
    # durable deletion record rather than creating a duplicate.
    from karma_team.runner.services.runner_delete import delete_project_runners

    delete_project_runners(target.id)
    assert ProjectDeletionTombstone.objects.filter(project_id=target.id).count() == 1


@pytest.mark.unit
def test_machine_snapshot_keeps_tombstone_after_project_hard_delete_and_scopes_workspace(
    db, api_client, create_user, workspace, project
):
    target = Project.objects.create(
        name="Durable tombstone target",
        identifier="DURABLE",
        workspace=workspace,
        created_by=create_user,
    )
    other_user = type(create_user).objects.create(
        username=f"tombstone-other-{uuid4().hex[:8]}",
        email=f"tombstone-other-{uuid4().hex[:8]}@example.com",
    )
    other_workspace = Workspace.objects.create(
        name="Tombstone Other",
        slug=f"tombstone-other-{uuid4().hex[:8]}",
        owner=other_user,
    )
    other_project_id = uuid4()
    ProjectDeletionTombstone.objects.create(
        project_id=other_project_id,
        workspace_id=other_workspace.id,
    )
    machine, raw_token = _machine_token(create_user=create_user, workspace=workspace)

    from karma_team.runner.services.runner_delete import delete_project_runners

    delete_project_runners(target.id)
    assert ProjectDeletionTombstone.objects.filter(project_id=target.id).count() == 1
    Project.all_objects.filter(pk=target.id).delete()
    assert not Project.all_objects.filter(pk=target.id).exists()

    response = _bearer(api_client, raw_token).get(_snapshot_url(machine))

    assert response.status_code == 200, response.data
    assert response.data["deleted_project_ids"] == [str(target.id)]
    assert str(other_project_id) not in response.data["deleted_project_ids"]


@pytest.mark.unit
def test_project_delete_rollback_does_not_emit_machine_remove(
    db, create_user, workspace, project, django_capture_on_commit_callbacks
):
    machine, _ = _machine_token(create_user=create_user, workspace=workspace)
    runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=Pod.default_for_project_id(project.id),
        name="rollback-project-delete",
        binding_source=RunnerBindingSource.AUTO_PROJECT_SYNC,
    )

    from karma_team.runner.services.runner_delete import delete_project_runners

    with patch("karma_team.runner.services.runner_delete.send_to_machine") as send_to_machine:
        with django_capture_on_commit_callbacks(execute=True) as callbacks:
            with pytest.raises(RuntimeError, match="force outer rollback"):
                with transaction.atomic():
                    delete_project_runners(project.id)
                    assert not Runner.objects.filter(pk=runner.id).exists()
                    raise RuntimeError("force outer rollback")

    assert callbacks == []
    send_to_machine.assert_not_called()
    assert Runner.objects.filter(pk=runner.id, revoked_at__isnull=True).exists()
    assert not ProjectDeletionTombstone.objects.filter(project_id=project.id).exists()
