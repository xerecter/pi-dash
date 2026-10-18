# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

# Django imports
from django.db.models import Q
from rest_framework import serializers

# Module imports
from karma_team.db.models import User, WorkspaceMemberInvite
from .base import BaseSerializer
from karma_team.app.permissions.base import ROLE


class WorkspaceInviteSerializer(BaseSerializer):
    """
    Serializer for workspace invites.
    """

    class Meta:
        model = WorkspaceMemberInvite
        fields = [
            "id",
            "email",
            "role",
            "created_at",
            "updated_at",
            "responded_at",
            "accepted",
        ]
        read_only_fields = [
            "id",
            "workspace",
            "created_at",
            "updated_at",
            "responded_at",
            "accepted",
        ]

    def validate_email(self, value):
        identifier = str(value or "").strip().lower()
        if not identifier:
            raise serializers.ValidationError("Account or email is required", code="ACCOUNT_IDENTIFIER_REQUIRED")
        user = (
            User.objects.filter(Q(email__iexact=identifier) | Q(username__iexact=identifier), is_active=True)
            .only("email")
            .first()
        )
        return (user.email if user and user.email else identifier).strip().lower()

    def validate_role(self, value):
        if value not in [ROLE.ADMIN.value, ROLE.MEMBER.value, ROLE.GUEST.value]:
            raise serializers.ValidationError("Invalid role", code="INVALID_WORKSPACE_MEMBER_ROLE")
        return value

    def validate(self, data):
        slug = self.context["slug"]
        if (
            data.get("email")
            and WorkspaceMemberInvite.objects.filter(email=data["email"], workspace__slug=slug).exists()
        ):
            raise serializers.ValidationError("Email already invited", code="EMAIL_ALREADY_INVITED")
        return data
