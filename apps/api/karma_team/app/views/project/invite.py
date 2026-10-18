# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

# Python imports
import jwt
from datetime import datetime

# Django imports
from django.conf import settings
from django.db.models import Q
from django.utils import timezone

# Third Party imports
from rest_framework.response import Response
from rest_framework import status
from rest_framework.permissions import AllowAny

# Module imports
from .base import BaseViewSet, BaseAPIView
from karma_team.app.serializers import ProjectMemberInviteSerializer
from karma_team.app.permissions import allow_permission, ROLE
from karma_team.db.models import (
    ProjectMember,
    Workspace,
    ProjectMemberInvite,
    User,
    WorkspaceMember,
    Project,
    ProjectUserProperty,
)
from karma_team.db.models.project import ProjectNetwork
from karma_team.utils.host import base_host
from karma_team.bgtasks.project_invitation_task import project_invitation


class ProjectInvitationsViewset(BaseViewSet):
    serializer_class = ProjectMemberInviteSerializer
    model = ProjectMemberInvite

    search_fields = []

    def get_queryset(self):
        return self.filter_queryset(
            super()
            .get_queryset()
            .filter(workspace__slug=self.kwargs.get("slug"))
            .filter(project_id=self.kwargs.get("project_id"))
            .select_related("project")
            .select_related("workspace", "workspace__owner")
        )

    @allow_permission([ROLE.ADMIN])
    def create(self, request, slug, project_id):
        emails = request.data.get("emails", [])

        # ``emails`` is the historical payload key. Each value may be either a
        # Karma username or an email-shaped identifier.
        if not emails:
            return Response({"error": "Accounts or emails are required"}, status=status.HTTP_400_BAD_REQUEST)

        normalized_invites = []
        for invite in emails:
            identifier = str(invite.get("email", "")).strip()
            if not identifier:
                return Response({"error": "Account or email is required"}, status=status.HTTP_400_BAD_REQUEST)
            user = (
                User.objects.filter(Q(email__iexact=identifier) | Q(username__iexact=identifier), is_active=True)
                .only("id", "email")
                .first()
            )
            normalized_invites.append(
                {
                    "email": (user.email if user and user.email else identifier).strip().lower(),
                    "role": invite.get("role", 5),
                    "user_id": user.id if user else None,
                }
            )
        emails = normalized_invites

        for email in emails:
            workspace_members = WorkspaceMember.objects.filter(workspace__slug=slug, is_active=True)
            if email["user_id"]:
                workspace_members = workspace_members.filter(member_id=email["user_id"])
            else:
                workspace_members = workspace_members.none()
            workspace_role = workspace_members.values_list("role", flat=True).first()

            if workspace_role in [5, 20] and workspace_role != email.get("role", 5):
                return Response({"error": "You cannot invite a user with different role than workspace role"})

        workspace = Workspace.objects.get(slug=slug)

        project_invitations = []
        for email in emails:
            project_invitations.append(
                ProjectMemberInvite(
                    email=email["email"],
                    project_id=project_id,
                    workspace_id=workspace.id,
                    token=jwt.encode(
                        {"email": email["email"], "timestamp": datetime.now().timestamp()},
                        settings.SECRET_KEY,
                        algorithm="HS256",
                    ),
                    role=email.get("role", 5),
                    created_by=request.user,
                )
            )

        # Create workspace member invite
        project_invitations = ProjectMemberInvite.objects.bulk_create(
            project_invitations, batch_size=10, ignore_conflicts=True
        )
        current_site = base_host(request=request, is_app=True)

        # Send invitations
        for invitation in project_invitations:
            project_invitation.delay(
                invitation.email,
                project_id,
                invitation.token,
                current_site,
                request.user.email,
            )

        return Response({"message": "Email sent successfully"}, status=status.HTTP_200_OK)


class UserProjectInvitationsViewset(BaseViewSet):
    serializer_class = ProjectMemberInviteSerializer
    model = ProjectMemberInvite

    def get_queryset(self):
        return self.filter_queryset(
            super()
            .get_queryset()
            .filter(email=self.request.user.email)
            .select_related("workspace", "workspace__owner", "project")
        )

    @allow_permission([ROLE.ADMIN, ROLE.MEMBER], level="WORKSPACE")
    def create(self, request, slug):
        project_ids = request.data.get("project_ids", [])

        # Get the workspace user role
        workspace_member = WorkspaceMember.objects.get(member=request.user, workspace__slug=slug, is_active=True)

        # Get all the projects
        projects = Project.objects.filter(id__in=project_ids, workspace__slug=slug).only("id", "network")
        # Check if user has permission to join each project
        for project in projects:
            if project.network == ProjectNetwork.SECRET.value and workspace_member.role != ROLE.ADMIN.value:
                return Response(
                    {"error": "Only workspace admins can join private project"},
                    status=status.HTTP_403_FORBIDDEN,
                )

        workspace_role = workspace_member.role
        workspace = workspace_member.workspace

        # If the user was already part of workspace
        _ = ProjectMember.objects.filter(workspace__slug=slug, project_id__in=project_ids, member=request.user).update(
            is_active=True
        )

        ProjectMember.objects.bulk_create(
            [
                ProjectMember(
                    project_id=project_id,
                    member=request.user,
                    role=workspace_role,
                    workspace=workspace,
                    created_by=request.user,
                )
                for project_id in project_ids
            ],
            ignore_conflicts=True,
        )

        ProjectUserProperty.objects.bulk_create(
            [
                ProjectUserProperty(
                    project_id=project_id,
                    user=request.user,
                    workspace=workspace,
                    created_by=request.user,
                )
                for project_id in project_ids
            ],
            ignore_conflicts=True,
        )

        return Response({"message": "Projects joined successfully"}, status=status.HTTP_201_CREATED)


class ProjectJoinEndpoint(BaseAPIView):
    permission_classes = [AllowAny]

    def post(self, request, slug, project_id, pk):
        project_invite = ProjectMemberInvite.objects.get(pk=pk, project_id=project_id, workspace__slug=slug)

        email = request.data.get("email", "")

        if email == "" or project_invite.email != email:
            return Response(
                {"error": "You do not have permission to join the project"},
                status=status.HTTP_403_FORBIDDEN,
            )

        if project_invite.responded_at is None:
            project_invite.accepted = request.data.get("accepted", False)
            project_invite.responded_at = timezone.now()
            project_invite.save()

            if project_invite.accepted:
                # Check if the user account exists
                user = User.objects.filter(email=email).first()

                # Check if user is a part of workspace
                workspace_member = WorkspaceMember.objects.filter(workspace__slug=slug, member=user).first()
                # Add him to workspace
                if workspace_member is None:
                    _ = WorkspaceMember.objects.create(
                        workspace_id=project_invite.workspace_id,
                        member=user,
                        role=(15 if project_invite.role >= 15 else project_invite.role),
                    )
                else:
                    # Else make him active
                    workspace_member.is_active = True
                    workspace_member.save()

                # Check if the user was already a member of project then activate the user
                project_member = ProjectMember.objects.filter(
                    workspace_id=project_invite.workspace_id, member=user
                ).first()
                if project_member is None:
                    # Create a Project Member
                    _ = ProjectMember.objects.create(
                        project_id=project_id,
                        member=user,
                        role=project_invite.role,
                    )
                else:
                    project_member.is_active = True
                    project_member.role = project_member.role
                    project_member.save()

                return Response(
                    {"message": "Project Invitation Accepted"},
                    status=status.HTTP_200_OK,
                )

            return Response(
                {"message": "Project Invitation was not accepted"},
                status=status.HTTP_200_OK,
            )

        return Response(
            {"error": "You have already responded to the invitation request"},
            status=status.HTTP_400_BAD_REQUEST,
        )

    def get(self, request, slug, project_id, pk):
        project_invitation = ProjectMemberInvite.objects.get(workspace__slug=slug, project_id=project_id, pk=pk)
        serializer = ProjectMemberInviteSerializer(project_invitation)
        return Response(serializer.data, status=status.HTTP_200_OK)
