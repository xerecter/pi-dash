# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import pytest
from django.urls import reverse
from rest_framework import status

from karma_team.db.models import User, WorkspaceMember, WorkspaceMemberInvite


@pytest.mark.contract
@pytest.mark.django_db
class TestWorkspaceInvitationIdentifiers:
    def test_accepts_username_and_uses_canonical_account_email(self, mocker, session_client, workspace):
        mocker.patch("karma_team.app.views.workspace.invite.workspace_invitation.delay")
        mocker.patch("karma_team.app.views.workspace.invite.track_event.delay")
        invited_user = User.objects.create(
            username="testfree1",
            email="testfree1-account",
            display_name="testfree1",
            is_active=True,
        )

        response = session_client.post(
            reverse("workspace-invitations", kwargs={"slug": workspace.slug}),
            {"emails": [{"email": "testfree1", "role": 15}]},
            format="json",
        )

        assert response.status_code == status.HTTP_200_OK
        invitation = WorkspaceMemberInvite.objects.get(workspace=workspace)
        assert invitation.email == invited_user.email
        assert invitation.role == 15

        session_client.force_authenticate(user=invited_user)
        response = session_client.post(
            reverse("user-workspace-invitations"),
            {"invitations": [str(invitation.id)]},
            format="json",
        )

        assert response.status_code == status.HTTP_204_NO_CONTENT
        assert WorkspaceMember.objects.filter(
            workspace=workspace, member=invited_user, role=15, is_active=True
        ).exists()
        assert not WorkspaceMemberInvite.objects.filter(pk=invitation.id).exists()

    @pytest.mark.parametrize(
        "identifier", ["plain-account-name", "team/user+east", "not-an-email@", "person@example.com"]
    )
    def test_accepts_identifier_without_format_validation(self, mocker, session_client, workspace, identifier):
        mocker.patch("karma_team.app.views.workspace.invite.workspace_invitation.delay")
        mocker.patch("karma_team.app.views.workspace.invite.track_event.delay")

        response = session_client.post(
            reverse("workspace-invitations", kwargs={"slug": workspace.slug}),
            {"emails": [{"email": identifier, "role": 15}]},
            format="json",
        )

        assert response.status_code == status.HTTP_200_OK
        assert WorkspaceMemberInvite.objects.get(workspace=workspace).email == identifier

    def test_blank_identifier_is_still_rejected(self, mocker, session_client, workspace):
        mocker.patch("karma_team.app.views.workspace.invite.workspace_invitation.delay")
        mocker.patch("karma_team.app.views.workspace.invite.track_event.delay")

        response = session_client.post(
            reverse("workspace-invitations", kwargs={"slug": workspace.slug}),
            {"emails": [{"email": "   ", "role": 15}]},
            format="json",
        )

        assert response.status_code == status.HTTP_400_BAD_REQUEST
        assert WorkspaceMemberInvite.objects.filter(workspace=workspace).count() == 0
