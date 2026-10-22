# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Read-only project/metadata tools."""

from __future__ import annotations

from pydantic_ai import RunContext

from karma_team.assistant.runtime.agent import assistant
from karma_team.assistant.runtime.deps import AssistantDeps
from karma_team.assistant.tools import _scoping
from karma_team.assistant.tools.labels import label_data
from karma_team.db.models import Label, ProjectMember


@assistant.tool
def list_projects(ctx: RunContext[AssistantDeps]) -> list[dict]:
    """List the projects in the current workspace that you can access."""
    projects = _scoping.member_projects(ctx.deps).order_by("name")[:50]
    return [
        {"id": str(p.id), "identifier": p.identifier, "name": p.name}
        for p in projects
    ]


@assistant.tool
def list_states(ctx: RunContext[AssistantDeps], project_id: str) -> list[dict]:
    """List the workflow states (e.g. Backlog, In Progress, Done) for a project."""
    states = _scoping.project_states(ctx.deps, project_id).order_by("sequence")
    return [
        {"id": str(s.id), "name": s.name, "group": s.group, "default": s.default}
        for s in states
    ]


@assistant.tool
def list_labels(ctx: RunContext[AssistantDeps], project_id: str) -> list[dict]:
    """List the labels available in a project."""
    _scoping.get_project(ctx.deps, project_id)  # scope check
    labels = Label.objects.filter(
        project_id=project_id, workspace__slug=ctx.deps.workspace_slug
    ).order_by("name")
    return [label_data(label) for label in labels]


@assistant.tool
def list_project_members(ctx: RunContext[AssistantDeps], project_id: str) -> list[dict]:
    """List project members by user_id, display_name, email, role and can_assign.
    Only users with can_assign=true may be added as work item assignees."""
    _scoping.get_project(ctx.deps, project_id)  # scope check
    members = ProjectMember.objects.filter(
        project_id=project_id, workspace__slug=ctx.deps.workspace_slug, is_active=True
    ).select_related("member")
    out = []
    for m in members:
        if m.member is None:
            continue
        out.append(
            {
                "user_id": str(m.member_id),
                "display_name": m.member.display_name or m.member.email or "",
                "email": m.member.email,
                "role": m.role,
                "can_assign": m.role >= 15 and m.member.is_active,
            }
        )
    return out
