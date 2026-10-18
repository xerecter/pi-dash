# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Account inputs accept arbitrary non-empty identifiers, while keeping auth checks."""

import uuid

import jwt
import pytest
from django.core.cache import cache
from django.test import Client
from django.urls import reverse
from django.utils import timezone

from karma_team.api.serializers.invite import WorkspaceInviteSerializer
from karma_team.db.models import ProjectMember, ProjectMemberInvite, User, WorkspaceMember
from karma_team.license.models import Instance, InstanceAdmin


PASSWORD = "A-very-strong-passphrase-72!karma"
pytestmark = [pytest.mark.contract, pytest.mark.django_db]


@pytest.fixture(autouse=True)
def account_instance(settings, monkeypatch):
    settings.WEB_URL = "http://localhost"
    settings.APP_BASE_URL = "http://localhost"
    settings.SPACE_BASE_URL = "http://localhost"
    settings.ADMIN_BASE_URL = "http://localhost"
    monkeypatch.setenv("EMAIL_HOST", "localhost")
    monkeypatch.setenv("ENABLE_SIGNUP", "1")
    monkeypatch.setenv("ENABLE_EMAIL_PASSWORD", "1")
    cache.clear()
    return Instance.objects.create(
        instance_name="Account tests",
        instance_id=str(uuid.uuid4()),
        current_version="1.0.0",
        last_checked_at=timezone.now(),
        is_setup_done=True,
    )


@pytest.mark.parametrize("surface", ["", "spaces/"])
@pytest.mark.parametrize("identifier", ["testfree1", "team/user+east", "not-an-email@", "user@example.com"])
def test_password_registration_accepts_identifiers(surface, identifier):
    client = Client(HTTP_USER_AGENT="account-test")
    response = client.post(f"/auth/{surface}sign-up/", {"email": f" {identifier} ", "password": PASSWORD})

    assert response.status_code == 302
    assert "error_code" not in response.url
    user = User.objects.get(email=identifier)
    assert user.check_password(PASSWORD)
    assert client.session["_auth_user_id"] == str(user.id)


@pytest.mark.parametrize("surface", ["", "spaces/"])
def test_registration_rejects_existing_username(surface):
    existing = User.objects.create(username="testfree1", email="canonical@example.com")
    response = Client().post(f"/auth/{surface}sign-up/", {"email": "TESTFREE1", "password": PASSWORD})

    assert "USER_ALREADY_EXIST" in response.url
    assert list(User.objects.values_list("id", flat=True)) == [existing.id]


@pytest.mark.parametrize("surface", ["", "spaces/"])
@pytest.mark.parametrize(
    "password,error", [(PASSWORD, "REQUIRED_EMAIL_PASSWORD_SIGN_UP"), ("weak", "PASSWORD_TOO_WEAK")]
)
def test_registration_keeps_required_and_password_checks(surface, password, error):
    identifier = "   " if password == PASSWORD else "testfree1"
    response = Client().post(f"/auth/{surface}sign-up/", {"email": identifier, "password": password})

    assert error in response.url
    assert not User.objects.exists()


@pytest.mark.parametrize("surface", ["", "spaces/"])
def test_password_recovery_resolves_username(api_client, mocker, surface):
    user = User.objects.create(username="testfree1", email="recovery@example.com")
    task = mocker.patch(
        f"karma_team.authentication.views.{'space' if surface else 'app'}.password_management.forgot_password.delay"
    )
    response = api_client.post(f"/auth/{surface}forgot-password/", {"email": " TESTFREE1 "}, format="json")

    assert response.status_code == 200
    assert task.call_args.args[1] == user.email


@pytest.mark.parametrize("surface", ["", "spaces/"])
def test_magic_code_resolves_username_and_requires_code(api_client, mocker, surface):
    user = User.objects.create(username="testfree1", email="delivery@example.com")
    task = mocker.patch(f"karma_team.authentication.views.{'space' if surface else 'app'}.magic.magic_link.delay")
    response = api_client.post(f"/auth/{surface}magic-generate/", {"email": " TESTFREE1 "}, format="json")

    assert response.status_code == 200
    assert task.call_args.args[0] == user.email
    response = Client().post(f"/auth/{surface}magic-sign-in/", {"email": "testfree1", "code": ""})
    assert "MAGIC_SIGN_IN_EMAIL_CODE_REQUIRED" in response.url


def test_account_change_accepts_identifier_and_still_verifies_code(session_client, create_user, mocker):
    send_code = mocker.patch("karma_team.app.views.user.base.send_email_update_magic_code.delay")
    mocker.patch("karma_team.app.views.user.base.send_email_update_confirmation.delay")
    response = session_client.post(reverse("user-email-verify-code"), {"email": " new-account "}, format="json")

    assert response.status_code == 200
    send_code.assert_called_once()
    identifier, code = send_code.call_args.args
    assert identifier == "new-account"
    response = session_client.patch(reverse("user-email-update"), {"email": identifier, "code": "wrong"}, format="json")
    assert response.status_code == 400
    create_user.refresh_from_db()
    assert create_user.email != identifier

    response = session_client.patch(reverse("user-email-update"), {"email": identifier, "code": code}, format="json")
    assert response.status_code == 200
    create_user.refresh_from_db()
    assert create_user.email == identifier
    assert User.objects.count() == 1


@pytest.mark.parametrize("identifier", ["   ", "existing-account"])
def test_account_change_rejects_empty_or_existing_username(session_client, identifier, mocker):
    User.objects.create(username="existing-account", email="existing@example.com")
    task = mocker.patch("karma_team.app.views.user.base.send_email_update_magic_code.delay")
    response = session_client.post(reverse("user-email-verify-code"), {"email": identifier}, format="json")

    assert response.status_code == 400
    task.assert_not_called()


@pytest.mark.parametrize("identifier", ["testfree1", "team/user+east", "not-an-email@", "user@example.com"])
def test_external_workspace_invite_serializer_accepts_identifier(workspace, identifier):
    serializer = WorkspaceInviteSerializer(data={"email": identifier, "role": 15}, context={"slug": workspace.slug})
    assert serializer.is_valid(), serializer.errors
    assert serializer.validated_data["email"] == identifier


def test_external_workspace_invite_serializer_resolves_username(workspace):
    User.objects.create(username="testfree1", email="canonical@example.com")
    serializer = WorkspaceInviteSerializer(data={"email": " TESTFREE1 ", "role": 15}, context={"slug": workspace.slug})
    assert serializer.is_valid(), serializer.errors
    assert serializer.validated_data["email"] == "canonical@example.com"


@pytest.mark.parametrize("identifier", ["testfree1", "team/user+east", "not-an-email@"])
def test_project_invitation_accepts_identifier(session_client, create_user, workspace, project, mocker, identifier):
    ProjectMember.objects.create(project=project, workspace=workspace, member=create_user, role=20)
    task = mocker.patch("karma_team.app.views.project.invite.project_invitation.delay")
    response = session_client.post(
        f"/api/workspaces/{workspace.slug}/projects/{project.id}/invitations/",
        {"emails": [{"email": identifier, "role": 15}]},
        format="json",
    )

    assert response.status_code == 200
    invite = ProjectMemberInvite.objects.get(project=project)
    assert invite.email == identifier
    assert jwt.decode(invite.token, options={"verify_signature": False})["email"] == identifier
    task.assert_called_once()


def test_project_invitation_resolves_username_and_preserves_role_check(
    session_client, create_user, workspace, project, mocker
):
    ProjectMember.objects.create(project=project, workspace=workspace, member=create_user, role=20)
    member = User.objects.create(username="testfree1", email="canonical@example.com")
    WorkspaceMember.objects.create(workspace=workspace, member=member, role=5)
    task = mocker.patch("karma_team.app.views.project.invite.project_invitation.delay")
    url = f"/api/workspaces/{workspace.slug}/projects/{project.id}/invitations/"
    response = session_client.post(url, {"emails": [{"email": "testfree1", "role": 15}]}, format="json")
    assert "error" in response.data
    task.assert_not_called()
    assert not ProjectMemberInvite.objects.exists()

    response = session_client.post(url, {"emails": [{"email": " TESTFREE1 ", "role": 5}]}, format="json")
    assert response.status_code == 200
    assert ProjectMemberInvite.objects.get(project=project).email == member.email
    task.assert_called_once()


def test_admin_setup_accepts_non_email_identifier(account_instance):
    client = Client(HTTP_USER_AGENT="account-test")
    response = client.post(
        "/api/instances/admins/sign-up/", {"email": " testfree1 ", "password": PASSWORD, "first_name": "Admin"}
    )

    assert response.status_code == 302
    assert "error_code" not in response.url
    admin = InstanceAdmin.objects.get(instance=account_instance).user
    assert admin.email == "testfree1"
    assert response.wsgi_request.session["_auth_user_id"] == str(admin.id)


@pytest.mark.parametrize(
    "admin,password,success", [(True, PASSWORD, True), (True, "wrong", False), (False, PASSWORD, False)]
)
def test_admin_login_resolves_username_and_checks_authority(account_instance, admin, password, success):
    user = User.objects.create(username="testfree1", email="admin@example.com")
    user.set_password(PASSWORD)
    user.save()
    if admin:
        InstanceAdmin.objects.create(instance=account_instance, user=user)
    client = Client(HTTP_USER_AGENT="account-test")
    response = client.post("/api/instances/admins/sign-in/", {"email": " TESTFREE1 ", "password": password})

    assert response.status_code == 302
    assert ("error_code" not in response.url) is success
    assert ("_auth_user_id" in response.wsgi_request.session) is success


def test_add_instance_admin_by_username(session_client, create_user, account_instance):
    InstanceAdmin.objects.create(instance=account_instance, user=create_user)
    member = User.objects.create(username="testfree1", email="canonical@example.com")
    response = session_client.post("/api/instances/admins/", {"email": " TESTFREE1 "}, format="json")

    assert response.status_code == 201
    assert InstanceAdmin.objects.filter(instance=account_instance, user=member).exists()
