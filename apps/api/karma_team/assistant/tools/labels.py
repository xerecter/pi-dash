# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only

"""Project label definitions; attaching labels uses the issue tools."""

from typing import Optional

from crum import impersonate
from django.db import IntegrityError, transaction
from pydantic_ai import ModelRetry, RunContext

from karma_team.assistant.runtime.agent import assistant
from karma_team.assistant.runtime.deps import AssistantDeps
from karma_team.assistant.tools import _issue_fields, _results, _scoping
from karma_team.db.models import Label, Project
from karma_team.utils.cache import invalidate_cache_directly


def label_data(label: Label) -> dict:
    return {
        "id": str(label.id),
        "project_id": str(label.project_id),
        "name": _results.wrap_untrusted(label.name),
        "color": label.color,
        "description": _results.wrap_untrusted(label.description),
        "parent_id": str(label.parent_id) if label.parent_id else None,
        "url_path": f"/{label.workspace.slug}/settings/projects/{label.project_id}/labels",
    }


def _get_label(deps, project_id: str, label_id: str) -> Label:
    project = _scoping.get_project(deps, project_id)
    pk = _issue_fields.uuid_list([label_id])[0]
    label = Label.objects.filter(pk=pk, project=project, workspace_id=deps.workspace_id).first()
    if label is None:
        raise _scoping.ToolNotFound("Label not found in this project.")
    return label


def _validate_name(project_id, name: str, exclude_id=None) -> str:
    name = name.strip()
    if not name or len(name) > 255:
        raise ModelRetry("A label name must contain between 1 and 255 characters.")
    if Label.objects.filter(project_id=project_id, name__iexact=name).exclude(pk=exclude_id).exists():
        raise ModelRetry("A label with this name already exists in the project.")
    return name


def _record(deps, label: Label, action: str) -> None:
    invalidate_cache_directly(path=f"/api/workspaces/{deps.workspace_slug}/labels/", user=False)
    path = f"/{deps.workspace_slug}/settings/projects/{label.project_id}/labels"
    _results.record_write(
        deps,
        f"{action} project label {label.name}",
        links=[
            {
                "type": "project_label",
                "workspace_slug": deps.workspace_slug,
                "project_id": str(label.project_id),
                "url_path": path,
            }
        ],
    )


@assistant.tool
def get_label(ctx: RunContext[AssistantDeps], project_id: str, label_id: str) -> dict:
    """Read a project label definition. Use get_issue to read labels attached to a work item."""
    return label_data(_get_label(ctx.deps, project_id, label_id))


@assistant.tool
def create_label(
    ctx: RunContext[AssistantDeps],
    project_id: str,
    name: str,
    color: str = "",
    description: str = "",
) -> dict:
    """Create a project label. Requires project Admin access. Then use update_issue_labels to attach it."""
    project = _scoping.get_project(ctx.deps, project_id)
    _scoping.require_project_admin(ctx.deps, project_id)
    if len(color) > 255:
        raise ModelRetry("Label color must be at most 255 characters.")
    try:
        with impersonate(_scoping.user_for(ctx.deps)), transaction.atomic():
            Project.objects.select_for_update().get(pk=project.pk)
            label = Label.objects.create(
                project=project,
                workspace_id=ctx.deps.workspace_id,
                name=_validate_name(project.id, name),
                color=color,
                description=description,
            )
    except IntegrityError as exc:
        raise ModelRetry("A label with this name already exists in the project.") from exc
    _record(ctx.deps, label, "Created")
    return {"created": True, **label_data(label)}


@assistant.tool
def update_label(
    ctx: RunContext[AssistantDeps],
    project_id: str,
    label_id: str,
    name: Optional[str] = None,
    color: Optional[str] = None,
    description: Optional[str] = None,
) -> dict:
    """Rename or edit a project label for all work items using it. Requires project Admin access.
    Omitted fields stay unchanged. To change one work item's labels, use update_issue_labels."""
    label = _get_label(ctx.deps, project_id, label_id)
    _scoping.require_project_admin(ctx.deps, project_id)
    if all(value is None for value in (name, color, description)):
        raise ModelRetry("Provide at least one label field to change.")
    if color is not None and len(color) > 255:
        raise ModelRetry("Label color must be at most 255 characters.")
    try:
        with impersonate(_scoping.user_for(ctx.deps)), transaction.atomic():
            Project.objects.select_for_update().get(pk=label.project_id)
            label = Label.objects.select_for_update().get(pk=label.pk)
            if name is not None:
                label.name = _validate_name(label.project_id, name, label.id)
            if color is not None:
                label.color = color
            if description is not None:
                label.description = description
            label.save(update_fields=["name", "color", "description", "updated_at", "updated_by"])
    except IntegrityError as exc:
        raise ModelRetry("A label with this name already exists in the project.") from exc
    _record(ctx.deps, label, "Updated")
    return {"updated": True, **label_data(label)}


@assistant.tool
def delete_label(ctx: RunContext[AssistantDeps], project_id: str, label_id: str) -> dict:
    """Delete a project label and its attachments using the normal soft-delete lifecycle.
    Requires project Admin access. To remove it from only one issue, use update_issue_labels instead."""
    _scoping.require_interactive_delete(ctx.deps)
    label = _get_label(ctx.deps, project_id, label_id)
    _scoping.require_project_admin(ctx.deps, project_id)
    with impersonate(_scoping.user_for(ctx.deps)), transaction.atomic():
        label = Label.objects.select_for_update().get(pk=label.pk)
        label.delete()
    _record(ctx.deps, label, "Deleted")
    return {"deleted": True, **label_data(label)}
