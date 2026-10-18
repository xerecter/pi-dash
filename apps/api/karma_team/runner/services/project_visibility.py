# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Project visibility shared by machine-token reconciliation endpoints."""

from __future__ import annotations

from django.db.models import Exists, OuterRef, QuerySet

from karma_team.core.permissions import ROLE_GUEST
from karma_team.db.models import ProjectMember, WorkspaceMember


def projects_visible_to_machine_token(queryset: QuerySet, token) -> QuerySet:
    """Scope projects to the token user's current workspace product access.

    Only active project members receive a runner, regardless of workspace
    role or project network setting. Public visibility alone does not grant
    permission to read or execute the project's work items.

    ``Exists``/``Subquery`` deliberately avoid joining membership tables into
    the outer query. Besides preventing duplicate projects, this lets the
    ensure endpoint safely apply ``SELECT FOR UPDATE OF project``: PostgreSQL
    rejects the tempting join + DISTINCT + FOR UPDATE formulation.
    """
    active_workspace = WorkspaceMember.objects.filter(
        workspace_id=token.workspace_id,
        member_id=token.user_id,
        is_active=True,
        role__gte=ROLE_GUEST,
    )
    project_membership = ProjectMember.objects.filter(
        project_id=OuterRef("pk"),
        workspace_id=token.workspace_id,
        member_id=token.user_id,
        is_active=True,
    )
    scoped = queryset.filter(
        workspace_id=token.workspace_id,
        deleted_at__isnull=True,
    ).annotate(
        _machine_active_workspace=Exists(active_workspace),
        _machine_project_member=Exists(project_membership),
    )
    return scoped.filter(_machine_active_workspace=True, _machine_project_member=True)
