# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Tests for the device-code (`karma-team auth login`) flow + CLI-initiated
runner mint + token revoke endpoints. See
``apps/api/karma_team/authentication/views/cli/device.py`` and
``apps/api/karma_team/runner/views/enrollment.py::RunnerCreateEndpoint``.
"""

from __future__ import annotations

import re
from concurrent.futures import ThreadPoolExecutor
from datetime import timedelta
from threading import Barrier
from unittest.mock import patch
from uuid import uuid4

import pytest
from django.db import close_old_connections, connection, transaction
from django.test import override_settings
from django.utils import timezone
from rest_framework.test import APIClient

from karma_team.authentication.services.cli_tokens import CLI_DEVICE_API_TOKEN_DESCRIPTION
from karma_team.db.models import APIToken, CLIDeviceCode, Project
from karma_team.runner.models import DevMachine, MachineToken, Pod, Runner, Visibility
from karma_team.runner.services import tokens as runner_tokens


# -------------------- device-code flow --------------------


@pytest.mark.unit
@override_settings(WEB_URL="http://api.example.test:28000", APP_BASE_URL="http://app.example.test:3000")
def test_device_start_returns_user_code_and_browser_verification_uri(db, api_client):
    resp = api_client.post("/api/v1/auth/device/start/", {}, format="json")
    assert resp.status_code == 200, resp.data
    assert resp.data["device_code"]
    assert resp.data["user_code"]
    assert "-" in resp.data["user_code"]
    assert resp.data["verification_uri"] == "http://app.example.test:3000/auth/device/"
    assert resp.data["expires_in"] > 0
    assert resp.data["interval"] > 0
    # Row created.
    assert CLIDeviceCode.objects.filter(device_code=resp.data["device_code"]).exists()


@pytest.mark.unit
def test_token_poll_pending_then_approved_mints_apitoken(db, api_client, session_client, create_user):
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    # First poll: pending (no approval yet).
    poll1 = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert poll1.status_code == 400
    assert poll1.data["error"] == "authorization_pending"

    # Operator approves via the web session.
    approve = session_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    assert approve.status_code == 200, approve.data
    assert approve.data["user_email"] == create_user.email

    # Second poll: success, returns a fresh APIToken.
    # Bump last_polled_at past the min-gap to skip slow_down.
    CLIDeviceCode.objects.filter(device_code=start["device_code"]).update(
        last_polled_at=timezone.now() - timedelta(seconds=30)
    )
    poll2 = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert poll2.status_code == 200, poll2.data
    assert poll2.data["access_token"].startswith("karma_team_api_")
    assert poll2.data["user_email"] == create_user.email
    minted = APIToken.objects.get(token=poll2.data["access_token"], user=create_user)
    assert re.fullmatch(r"karma-team CLI · \d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC", minted.label), minted.label
    # Row marked consumed.
    row = CLIDeviceCode.objects.get(device_code=start["device_code"])
    assert row.consumed is True


@pytest.mark.unit
def test_token_poll_rejects_consumed_code(db, api_client, session_client):
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    session_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    CLIDeviceCode.objects.filter(device_code=start["device_code"]).update(
        last_polled_at=timezone.now() - timedelta(seconds=30)
    )
    first = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert first.status_code == 200, first.data
    # Second exchange of the same device code must fail.
    second = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert second.status_code == 400
    assert second.data["error"] == "invalid_grant"


@pytest.mark.unit
def test_token_poll_returns_slow_down_when_polled_too_fast(db, api_client):
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    first = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert first.status_code == 400
    # Immediate second poll → slow_down.
    second = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert second.status_code == 400
    assert second.data["error"] == "slow_down"


@pytest.mark.unit
def test_token_poll_expired(db, api_client):
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    CLIDeviceCode.objects.filter(device_code=start["device_code"]).update(
        expires_at=timezone.now() - timedelta(seconds=1)
    )
    resp = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert resp.status_code == 410
    assert resp.data["error"] == "expired_token"


@pytest.mark.unit
def test_approve_rejects_unknown_code(db, session_client):
    resp = session_client.post("/api/v1/auth/device/approve/", {"user_code": "ZZZZ-9999"}, format="json")
    assert resp.status_code == 404


@pytest.mark.unit
def test_approve_normalizes_dashes_and_case(db, api_client, session_client):
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    code = start["user_code"].replace("-", "").lower()
    resp = session_client.post("/api/v1/auth/device/approve/", {"user_code": code}, format="json")
    assert resp.status_code == 200, resp.data


# -------------------- revoke --------------------


@pytest.mark.unit
def test_revoke_marks_token_inactive_idempotently(db, api_key_client, api_token):
    resp = api_key_client.post("/api/v1/auth/revoke/", {}, format="json")
    assert resp.status_code == 200
    api_token.refresh_from_db()
    assert api_token.is_active is False
    # Second call still 200 (token already inactive; the call itself
    # fails auth, so we test via re-activating then calling once more).
    api_token.is_active = True
    api_token.save(update_fields=["is_active"])
    resp = api_key_client.post("/api/v1/auth/revoke/", {}, format="json")
    assert resp.status_code == 200


@pytest.mark.unit
def test_machine_token_exchange_consumes_device_flow_api_token(db, api_client, create_user, workspace):
    api_token = APIToken.objects.create(
        user=create_user,
        workspace=workspace,
        user_type=0,
        label="karma-team CLI bridge",
        description=CLI_DEVICE_API_TOKEN_DESCRIPTION,
    )
    dev_machine_id = uuid4()
    api_client.credentials(HTTP_X_API_KEY=api_token.token)

    resp = api_client.post(
        "/api/v1/auth/machine-token/",
        {
            "workspace_slug": workspace.slug,
            "dev_machine_id": str(dev_machine_id),
            "host_label": "test-host",
        },
        format="json",
    )

    assert resp.status_code == 201, resp.data
    assert resp.data["machine_token"].startswith("mt_")
    api_token.refresh_from_db()
    assert api_token.is_active is False
    assert MachineToken.objects.filter(
        user=create_user,
        workspace=workspace,
        dev_machine_id=dev_machine_id,
        revoked_at__isnull=True,
    ).exists()


# -------------------- CLI-initiated runner mint --------------------


@pytest.mark.unit
def test_runner_create_succeeds_with_api_key(db, api_key_client, create_user, workspace, project):
    dev_machine_id = uuid4()
    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": workspace.slug,
            "project": project.identifier,
            "dev_machine_id": str(dev_machine_id),
            "host_label": "test-host",
        },
        format="json",
    )
    assert resp.status_code == 201, resp.data
    body = resp.data
    assert body["runner_id"]
    assert body["refresh_token"] == ""
    assert body["access_token"] == ""
    assert body["machine_token"].startswith("mt_")
    assert body["machine_token_minted"] is True
    assert body["workspace_slug"] == workspace.slug
    assert body["project_identifier"] == project.identifier
    assert body["protocol_version"] == 4
    runner = Runner.objects.get(id=body["runner_id"])
    assert runner.visibility == Visibility.PRIVATE
    assert runner.dev_machine is not None
    assert runner.dev_machine_id == dev_machine_id
    assert runner.dev_machine.owner_id == create_user.id
    assert runner.dev_machine.host_label == "test-host"
    assert runner.dev_machine.visibility == Visibility.PRIVATE
    assert DevMachine.objects.filter(owner=create_user, id=dev_machine_id).count() == 1
    assert (
        MachineToken.objects.filter(
            user=create_user,
            workspace=workspace,
            dev_machine_id=dev_machine_id,
            revoked_at__isnull=True,
        ).count()
        == 1
    )

    session = api_key_client.post(
        f"/api/v1/runner/runners/{runner.id}/sessions/",
        {
            "version": "test",
            "os": "linux",
            "arch": "x86_64",
            "status": "idle",
            "project_slug": project.identifier,
            "host_label": "test-host",
            "working_dir": "/home/dev/projects/acme",
            "agent_versions": {},
        },
        format="json",
        HTTP_AUTHORIZATION=f"Bearer {body['machine_token']}",
        HTTP_X_RUNNER_PROTOCOL_VERSION="4",
    )
    assert session.status_code == 201, session.data
    assert session.data["welcome"]["rid"] == str(runner.id)
    # Session-open persists the runner's local dev-machine working directory
    # so it can be surfaced in runner detail.
    runner.refresh_from_db()
    assert runner.dev_metadata == {"working_dir": "/home/dev/projects/acme"}


@pytest.mark.unit
def test_runner_create_with_machine_token_reuses_shared_dev_machine_token(
    db, api_client, create_user, workspace, project
):
    dev_machine_id = uuid4()
    DevMachine.objects.create(
        id=dev_machine_id,
        owner=create_user,
        host_label="shared-host",
        label="shared-host",
    )
    raw_machine_token = "mt_shared_test_token"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine_id=dev_machine_id,
        host_label="shared-host",
        token_hash=runner_tokens.hash_token(raw_machine_token),
        token_fingerprint=runner_tokens.fingerprint(raw_machine_token),
        label="machine: shared-host",
        is_service=True,
    )

    for name in ["runner_a", "runner_b"]:
        resp = api_client.post(
            "/api/v1/runner/runners/",
            {
                "workspace_slug": workspace.slug,
                "project": project.identifier,
                "dev_machine_id": str(dev_machine_id),
                "host_label": "shared-host",
                "name": name,
            },
            format="json",
            HTTP_X_API_KEY=raw_machine_token,
        )
        assert resp.status_code == 201, resp.data
        assert resp.data["refresh_token"] == ""
        assert resp.data["access_token"] == ""
        assert resp.data["machine_token_minted"] is False
        assert "machine_token" not in resp.data

    assert (
        MachineToken.objects.filter(
            user=create_user,
            workspace=workspace,
            dev_machine_id=dev_machine_id,
            revoked_at__isnull=True,
        ).count()
        == 1
    )
    assert set(Runner.objects.filter(name__in=["runner_a", "runner_b"]).values_list("dev_machine_id", flat=True)) == {
        dev_machine_id,
    }
    assert not Runner.objects.filter(
        name__in=["runner_a", "runner_b"],
        creation_request_id__isnull=False,
    ).exists()


@pytest.mark.unit
def test_runner_create_request_id_replay_returns_original_runner(
    db, api_client, create_user, workspace, project
):
    dev_machine_id = uuid4()
    machine = DevMachine.objects.create(
        id=dev_machine_id,
        owner=create_user,
        host_label="idempotent-host",
        label="idempotent-host",
    )
    raw_machine_token = "mt_idempotent_create"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(raw_machine_token),
        token_fingerprint=runner_tokens.fingerprint(raw_machine_token),
        label="machine: idempotent-host",
        is_service=True,
    )
    request_id = uuid4()
    payload = {
        "workspace_slug": workspace.slug,
        "project": project.identifier,
        "dev_machine_id": str(machine.id),
        "host_label": machine.host_label,
        "name": "idempotent_runner",
        "request_id": str(request_id),
    }

    first = api_client.post(
        "/api/v1/runner/runners/",
        payload,
        format="json",
        HTTP_X_API_KEY=raw_machine_token,
    )
    replay = api_client.post(
        "/api/v1/runner/runners/",
        {**payload, "name": "ignored_on_replay"},
        format="json",
        HTTP_X_API_KEY=raw_machine_token,
    )

    assert first.status_code == 201, first.data
    assert replay.status_code == 200, replay.data
    assert replay.data["runner_id"] == first.data["runner_id"]
    assert Runner.objects.filter(
        dev_machine=machine,
        creation_request_id=request_id,
    ).count() == 1


@pytest.mark.django_db(transaction=True)
def test_runner_create_request_id_replay_does_not_lock_related_project(
    create_user, workspace, project
):
    """Replay must not invert the Project -> Runner deletion lock order."""

    if connection.vendor != "postgresql":
        pytest.skip("row-lock concurrency contract requires PostgreSQL")

    machine = DevMachine.objects.create(
        id=uuid4(),
        owner=create_user,
        host_label="replay-lock-scope",
        label="replay-lock-scope",
    )
    raw_machine_token = "mt_replay_lock_scope"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(raw_machine_token),
        token_fingerprint=runner_tokens.fingerprint(raw_machine_token),
        label="machine: replay-lock-scope",
        is_service=True,
    )
    request_id = uuid4()
    runner = Runner.objects.create(
        owner=create_user,
        workspace=workspace,
        dev_machine=machine,
        pod=Pod.default_for_project(project),
        name="replay_lock_scope",
        creation_request_id=request_id,
        enrolled_at=timezone.now(),
    )
    payload = {
        "workspace_slug": workspace.slug,
        "project": project.identifier,
        "dev_machine_id": str(machine.id),
        "host_label": machine.host_label,
        "request_id": str(request_id),
    }

    def replay_once():
        close_old_connections()
        try:
            # Fail quickly if a regression makes the replay query lock the
            # select_related Project row held by the main transaction.
            with connection.cursor() as cursor:
                cursor.execute("SET lock_timeout = '750ms'")
            client = APIClient()
            client.credentials(HTTP_X_API_KEY=raw_machine_token)
            response = client.post(
                "/api/v1/runner/runners/",
                payload,
                format="json",
            )
            return response.status_code, dict(response.data)
        finally:
            close_old_connections()

    with transaction.atomic():
        Project.objects.select_for_update(of=("self",)).get(pk=project.pk)
        with ThreadPoolExecutor(max_workers=1) as executor:
            status_code, body = executor.submit(replay_once).result(timeout=5)

    assert status_code == 200, body
    assert body["runner_id"] == str(runner.id)


@pytest.mark.unit
def test_runner_create_request_id_cannot_cross_workspace_scope(
    db, api_client, create_user, workspace, project
):
    from karma_team.db.models.workspace import Workspace, WorkspaceMember

    machine = DevMachine.objects.create(
        id=uuid4(),
        owner=create_user,
        host_label="cross-workspace-request",
        label="cross-workspace-request",
    )
    first_raw_token = "mt_first_workspace_request"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(first_raw_token),
        token_fingerprint=runner_tokens.fingerprint(first_raw_token),
        label="machine: first workspace",
        is_service=True,
    )
    other_workspace = Workspace.objects.create(
        name="Request Scope Workspace",
        slug=f"request-scope-{uuid4().hex[:8]}",
        owner=create_user,
    )
    WorkspaceMember.objects.create(
        workspace=other_workspace,
        member=create_user,
        role=20,
    )
    other_project = Project.objects.create(
        name="Request Scope Project",
        identifier="REQSCOPE",
        workspace=other_workspace,
        created_by=create_user,
    )
    second_raw_token = "mt_second_workspace_request"
    MachineToken.objects.create(
        user=create_user,
        workspace=other_workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(second_raw_token),
        token_fingerprint=runner_tokens.fingerprint(second_raw_token),
        label="machine: second workspace",
        is_service=True,
    )
    request_id = uuid4()

    first = api_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": workspace.slug,
            "project": project.identifier,
            "dev_machine_id": str(machine.id),
            "request_id": str(request_id),
        },
        format="json",
        HTTP_X_API_KEY=first_raw_token,
    )
    conflict = api_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": other_workspace.slug,
            "project": other_project.identifier,
            "dev_machine_id": str(machine.id),
            "request_id": str(request_id),
        },
        format="json",
        HTTP_X_API_KEY=second_raw_token,
    )

    assert first.status_code == 201, first.data
    assert conflict.status_code == 409, conflict.data
    assert conflict.data["error"] == "idempotency_scope_conflict"
    assert Runner.objects.filter(
        dev_machine=machine,
        creation_request_id=request_id,
    ).count() == 1


@pytest.mark.unit
def test_runner_create_rejects_invalid_request_id(
    db, api_key_client, workspace, project
):
    response = api_key_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": workspace.slug,
            "project": project.identifier,
            "dev_machine_id": str(uuid4()),
            "host_label": "invalid-request-id",
            "request_id": "not-a-uuid",
        },
        format="json",
    )

    assert response.status_code == 400, response.data
    assert response.data["error"] == "invalid_request_id"


@pytest.mark.unit
def test_runner_create_different_request_ids_create_distinct_runners(
    db, api_client, create_user, workspace, project
):
    machine = DevMachine.objects.create(
        id=uuid4(),
        owner=create_user,
        host_label="distinct-requests",
        label="distinct-requests",
    )
    raw_machine_token = "mt_distinct_create_requests"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(raw_machine_token),
        token_fingerprint=runner_tokens.fingerprint(raw_machine_token),
        label="machine: distinct-requests",
        is_service=True,
    )
    base_payload = {
        "workspace_slug": workspace.slug,
        "project": project.identifier,
        "dev_machine_id": str(machine.id),
        "host_label": machine.host_label,
    }

    responses = []
    for request_id in (uuid4(), uuid4()):
        responses.append(
            api_client.post(
                "/api/v1/runner/runners/",
                {**base_payload, "request_id": str(request_id)},
                format="json",
                HTTP_X_API_KEY=raw_machine_token,
            )
        )

    assert [response.status_code for response in responses] == [201, 201]
    assert len({response.data["runner_id"] for response in responses}) == 2
    assert Runner.objects.filter(
        dev_machine=machine,
        creation_request_id__isnull=False,
    ).count() == 2


@pytest.mark.django_db(transaction=True)
def test_concurrent_runner_create_request_id_replays_one_runner(
    create_user, workspace, project
):
    if connection.vendor != "postgresql":
        pytest.skip("row-lock concurrency contract requires PostgreSQL")

    machine = DevMachine.objects.create(
        id=uuid4(),
        owner=create_user,
        host_label="concurrent-create",
        label="concurrent-create",
    )
    raw_machine_token = "mt_concurrent_create_request"
    MachineToken.objects.create(
        user=create_user,
        workspace=workspace,
        dev_machine=machine,
        host_label=machine.host_label,
        token_hash=runner_tokens.hash_token(raw_machine_token),
        token_fingerprint=runner_tokens.fingerprint(raw_machine_token),
        label="machine: concurrent-create",
        is_service=True,
    )
    request_id = uuid4()
    payload = {
        "workspace_slug": workspace.slug,
        "project": project.identifier,
        "dev_machine_id": str(machine.id),
        "host_label": machine.host_label,
        "name": "concurrent_idempotent_runner",
        "request_id": str(request_id),
    }
    worker_count = 8
    barrier = Barrier(worker_count)

    def create_once():
        close_old_connections()
        client = APIClient()
        client.credentials(HTTP_X_API_KEY=raw_machine_token)
        barrier.wait(timeout=10)
        response = client.post(
            "/api/v1/runner/runners/",
            payload,
            format="json",
        )
        result = (response.status_code, dict(response.data))
        close_old_connections()
        return result

    with ThreadPoolExecutor(max_workers=worker_count) as executor:
        results = list(executor.map(lambda _: create_once(), range(worker_count)))

    statuses = [status_code for status_code, _ in results]
    assert statuses.count(201) == 1, results
    assert all(status_code in {200, 201} for status_code in statuses), results
    assert len({body["runner_id"] for _, body in results}) == 1
    assert Runner.objects.filter(
        dev_machine=machine,
        creation_request_id=request_id,
    ).count() == 1


@pytest.mark.unit
def test_runner_create_consumes_legacy_cli_api_token(db, api_client, create_user, workspace, project):
    api_token = APIToken.objects.create(
        user=create_user,
        workspace=workspace,
        user_type=0,
        label="karma-team CLI bridge",
        description=CLI_DEVICE_API_TOKEN_DESCRIPTION,
    )
    api_client.credentials(HTTP_X_API_KEY=api_token.token)

    resp = api_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": workspace.slug,
            "project": project.identifier,
            "dev_machine_id": str(uuid4()),
            "host_label": "test-host",
            "name": "legacy_cli_token_runner",
        },
        format="json",
    )

    assert resp.status_code == 201, resp.data
    assert resp.data["machine_token"].startswith("mt_")
    api_token.refresh_from_db()
    assert api_token.is_active is False


@pytest.mark.unit
def test_runner_create_uses_stable_dev_machine_id_not_host_label(db, api_key_client, create_user, workspace, project):
    first_machine_id = uuid4()
    second_machine_id = uuid4()
    for dev_machine_id, name in [
        (first_machine_id, "runner_a"),
        (second_machine_id, "runner_b"),
    ]:
        resp = api_key_client.post(
            "/api/v1/runner/runners/",
            {
                "workspace_slug": workspace.slug,
                "project": project.identifier,
                "dev_machine_id": str(dev_machine_id),
                "host_label": "same-hostname",
                "name": name,
            },
            format="json",
        )
        assert resp.status_code == 201, resp.data

    assert DevMachine.objects.filter(owner=create_user, host_label="same-hostname").count() == 2
    assert set(Runner.objects.filter(name__in=["runner_a", "runner_b"]).values_list("dev_machine_id", flat=True)) == {
        first_machine_id,
        second_machine_id,
    }


@pytest.mark.unit
def test_runner_create_404_for_unknown_workspace(db, api_key_client, project):
    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {"workspace_slug": "nope", "project": project.identifier},
        format="json",
    )
    assert resp.status_code == 404


@pytest.mark.unit
def test_runner_create_404_for_unknown_project(db, api_key_client, workspace):
    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {"workspace_slug": workspace.slug, "project": "NOPE"},
        format="json",
    )
    assert resp.status_code == 404


@pytest.mark.unit
def test_runner_create_revalidates_active_project_after_machine_lock(
    db, api_key_client, create_user, workspace, project
):
    """A project deletion that wins during setup must not leave an orphan."""
    machine = DevMachine.objects.create(
        id=uuid4(),
        owner=create_user,
        host_label="project-delete-race",
    )

    def delete_project_then_return_machine(**_kwargs):
        Project.all_objects.filter(pk=project.id).update(deleted_at=timezone.now())
        return machine

    with patch(
        "karma_team.runner.views.enrollment._get_or_create_dev_machine",
        side_effect=delete_project_then_return_machine,
    ):
        response = api_key_client.post(
            "/api/v1/runner/runners/",
            {
                "workspace_slug": workspace.slug,
                "project": project.identifier,
                "dev_machine_id": str(machine.id),
                "host_label": machine.host_label,
                "name": "must-not-be-orphaned",
            },
            format="json",
        )

    assert response.status_code == 404, response.data
    assert response.data["error"] == "project_not_found"
    assert not Runner.objects.filter(dev_machine=machine).exists()


@pytest.mark.unit
def test_runner_create_requires_auth(db, api_client, workspace, project):
    # No X-Api-Key header at all → 401 from APIKeyAuthentication.
    resp = api_client.post(
        "/api/v1/runner/runners/",
        {"workspace_slug": workspace.slug, "project": project.identifier},
        format="json",
    )
    # DRF returns 403 when no auth class produces a credential (unauthenticated).
    assert resp.status_code in (401, 403)


@pytest.mark.unit
def test_runner_create_infers_workspace_when_caller_has_one(db, api_key_client, workspace, project):
    """The documented onboarding flow: `karma-team runner add --project X`
    (without --workspace) must succeed when the caller belongs to
    exactly one workspace. The `workspace` fixture wires a single
    membership for `create_user`, matching the dev-laptop case.
    """
    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {"project": project.identifier, "host_label": "test-host"},
        format="json",
    )
    assert resp.status_code == 201, resp.data
    assert resp.data["workspace_slug"] == workspace.slug


@pytest.mark.unit
def test_runner_create_400_when_workspace_ambiguous(db, api_key_client, create_user, workspace, project):
    """If the caller belongs to multiple workspaces and omits
    `workspace_slug`, we should refuse with a specific error rather
    than guessing wrong.
    """
    from karma_team.db.models.workspace import Workspace, WorkspaceMember

    other_ws = Workspace.objects.create(
        name="other-ws",
        owner=create_user,
        slug="other-ws",
    )
    WorkspaceMember.objects.create(workspace=other_ws, member=create_user, role=20)

    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {"project": project.identifier},
        format="json",
    )
    assert resp.status_code == 400
    assert resp.data["error"] == "workspace_slug_required"


@pytest.mark.unit
def test_runner_create_rejects_invalid_name(db, api_key_client, workspace, project):
    """Disallow path-traversal / control chars in user-supplied names."""
    resp = api_key_client.post(
        "/api/v1/runner/runners/",
        {
            "workspace_slug": workspace.slug,
            "project": project.identifier,
            "name": "../etc/passwd",
        },
        format="json",
    )
    assert resp.status_code == 400
    assert resp.data["error"] == "invalid_runner_name"


@pytest.mark.unit
def test_runner_create_400_when_no_workspace_membership(db, api_client, create_user, workspace, project):
    """A token bound to a user with no workspace memberships at all
    should get a clear `no_workspace_membership` error, not a 500.
    """
    from karma_team.db.models import APIToken
    from karma_team.db.models.workspace import WorkspaceMember

    WorkspaceMember.objects.filter(member=create_user).delete()
    # Mint a fresh token now that the user has no memberships.
    tok = APIToken.objects.create(user=create_user)
    api_client.credentials(HTTP_X_API_KEY=tok.token)

    resp = api_client.post(
        "/api/v1/runner/runners/",
        {"project": project.identifier},
        format="json",
    )
    assert resp.status_code == 400
    assert resp.data["error"] == "no_workspace_membership"


# -------------------- workspaces list --------------------


@pytest.mark.unit
def test_workspaces_list_returns_single_membership(db, api_key_client, workspace):
    """Single-workspace caller sees exactly that workspace, by slug + name.

    Drives the no-prompt branch of `karma-team auth login`'s workspace
    resolver: one membership → silently persist `[cli].workspace_slug`.
    """
    resp = api_key_client.get("/api/v1/auth/workspaces/")
    assert resp.status_code == 200, resp.data
    assert resp.data == {"workspaces": [{"slug": workspace.slug, "name": workspace.name}]}


@pytest.mark.unit
def test_workspaces_list_returns_multiple_in_join_order(db, api_key_client, create_user, workspace):
    """Multi-workspace caller sees every active membership, ordered by
    join time. The CLI's picker renders them in this order so the list
    is stable across calls.
    """
    from karma_team.db.models.workspace import Workspace, WorkspaceMember

    second = Workspace.objects.create(name="second-ws", owner=create_user, slug="second-ws")
    WorkspaceMember.objects.create(workspace=second, member=create_user, role=20)

    resp = api_key_client.get("/api/v1/auth/workspaces/")
    assert resp.status_code == 200, resp.data
    slugs = [w["slug"] for w in resp.data["workspaces"]]
    assert slugs == [workspace.slug, second.slug]


@pytest.mark.unit
def test_workspaces_list_excludes_inactive_memberships(db, api_key_client, create_user, workspace):
    """Soft-removed memberships (`is_active=False`) must not surface
    in the picker — otherwise the user could pick a workspace they no
    longer have access to and hit a confusing 403 on the next call.
    """
    from karma_team.db.models.workspace import Workspace, WorkspaceMember

    inactive = Workspace.objects.create(name="gone-ws", owner=create_user, slug="gone-ws")
    WorkspaceMember.objects.create(workspace=inactive, member=create_user, role=20, is_active=False)

    resp = api_key_client.get("/api/v1/auth/workspaces/")
    assert resp.status_code == 200, resp.data
    slugs = [w["slug"] for w in resp.data["workspaces"]]
    assert slugs == [workspace.slug]


@pytest.mark.unit
def test_workspaces_list_requires_auth(db, api_client):
    """Anonymous callers get 401; the endpoint is CLI-token only."""
    resp = api_client.get("/api/v1/auth/workspaces/")
    assert resp.status_code in (401, 403)


# -------------------- approval hardening --------------------


@pytest.mark.unit
def test_approve_rejects_second_user_takeover(db, api_client, session_client, create_user):
    """Once user A approves a code, user B (a different logged-in
    session) cannot re-approve and steal the eventual token.
    """
    from karma_team.db.models import User

    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    # User A approves.
    first = session_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    assert first.status_code == 200, first.data

    # User B (different account) tries to approve the same code.
    user_b = User.objects.create(email="b@example.com", username="b")
    api_client.force_authenticate(user=user_b)
    second = api_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    assert second.status_code == 409
    api_client.force_authenticate(user=None)

    # Subsequent CLI poll mints the token for user A (the original
    # approver), not user B.
    CLIDeviceCode.objects.filter(device_code=start["device_code"]).update(
        last_polled_at=timezone.now() - timedelta(seconds=30)
    )
    poll = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert poll.status_code == 200
    assert poll.data["user_email"] == create_user.email


@pytest.mark.unit
def test_approve_idempotent_for_same_user(db, api_client, session_client, create_user):
    """Re-approving by the same user is OK (idempotent UX): the human
    might double-click Approve in the browser.
    """
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    a = session_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    assert a.status_code == 200
    b = session_client.post("/api/v1/auth/device/approve/", {"user_code": start["user_code"]}, format="json")
    assert b.status_code == 200


# -------------------- slow_down DoS prevention --------------------


@pytest.mark.unit
def test_slow_down_does_not_update_last_polled_at(db, api_client):
    """A rapid poller (or attacker holding the device_code) must NOT be
    able to slip `last_polled_at` forward on each rejection, otherwise
    the legit CLI's slower poll would always look "too fast" too.
    """
    start = api_client.post("/api/v1/auth/device/start/", {}, format="json").data
    # First poll establishes last_polled_at.
    first = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert first.status_code == 400
    row = CLIDeviceCode.objects.get(device_code=start["device_code"])
    first_polled = row.last_polled_at
    assert first_polled is not None

    # Second poll within the min-gap returns slow_down — and must leave
    # last_polled_at frozen at the previous value.
    second = api_client.post("/api/v1/auth/device/token/", {"device_code": start["device_code"]}, format="json")
    assert second.status_code == 400
    assert second.data["error"] == "slow_down"
    row.refresh_from_db()
    assert row.last_polled_at == first_polled
