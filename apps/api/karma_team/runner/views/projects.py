# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Project listing for runner registration / discovery.

Two access modes:

- **Runner access-token auth** — used by the daemon so the CLI / TUI
  can show a user the projects available for registration. Scoped to
  the runner's workspace.
- **Session auth** — used by the cloud web UI when offering a project
  picker on a "create runner" or pod-creation form. Scoped to the
  user's workspace membership.
"""

from __future__ import annotations

import uuid

from rest_framework import status
from rest_framework.response import Response
from rest_framework.views import APIView

from karma_team.api.middleware.api_authentication import APIKeyAuthentication
from karma_team.authentication.session import BaseSessionAuthentication
from karma_team.db.models.project import Project
from karma_team.db.models.workspace import WorkspaceMember
from karma_team.runner.authentication import (
    MachineTokenAuthentication,
    RunnerAccessTokenAuthentication,
)
from karma_team.runner.models import Pod, ProjectDeletionTombstone, Runner
from karma_team.runner.services.project_visibility import (
    projects_visible_to_machine_token,
)


def _serialize_projects(workspace_id) -> list[dict]:
    """Return one row per project in ``workspace_id`` with pod counts and
    the embedded pod list.

    The TUI's add-runner form needs the pod list per project so it can
    render a cascaded picker (project → pod) without a second
    round-trip on every project change. Pods are tiny (a few per
    project), so embedding is cheaper than chatty cascade fetches.

    Each project includes a ``pods`` array sorted with the default pod
    first; ``default_pod_id`` and ``pod_count`` stay on the parent for
    callers that don't care about the full list.
    """
    pods_by_project: dict = {}
    default_pod_ids: dict = {}
    for row in (
        Pod.objects.filter(workspace_id=workspace_id)
        .values("project_id", "is_default", "id", "name")
        .order_by("-is_default", "name")
    ):
        pid = row["project_id"]
        pods_by_project.setdefault(pid, []).append(
            {
                "id": str(row["id"]),
                "name": row["name"],
                "is_default": bool(row["is_default"]),
            }
        )
        if row["is_default"] and pid not in default_pod_ids:
            default_pod_ids[pid] = row["id"]
    return [
        {
            "id": str(p.id),
            "identifier": p.identifier,
            "name": p.name,
            "description": p.description,
            "is_default": p.is_default,
            "default_pod_id": (
                str(default_pod_ids[p.id]) if p.id in default_pod_ids else None
            ),
            "pod_count": len(pods_by_project.get(p.id, [])),
            "pods": pods_by_project.get(p.id, []),
        }
        for p in Project.objects.filter(workspace_id=workspace_id).order_by(
            "identifier"
        )
    ]


class ProjectListEndpoint(APIView):
    """GET /api/runners/projects/

    Three auth modes (in order):
    - Bearer access-token (runner) — scoped to ``runner.workspace_id``.
    - ``X-Api-Key`` (CLI/user) — scoped to caller's workspace memberships.
      Used by ``karma-team auth login``'s post-login project picker.
    - Session auth (web UI) — scoped to caller's workspace memberships.
    """

    authentication_classes = [
        RunnerAccessTokenAuthentication,
        APIKeyAuthentication,
        BaseSessionAuthentication,
    ]
    permission_classes: list = []

    def get(self, request):
        runner = getattr(request, "auth_runner", None)
        if runner is not None:
            return Response(_serialize_projects(runner.workspace_id))

        if not getattr(request.user, "is_authenticated", False):
            return Response(
                {"error": "authentication required"},
                status=status.HTTP_401_UNAUTHORIZED,
            )
        ws_filter = request.query_params.get("workspace")
        if ws_filter:
            if not WorkspaceMember.objects.filter(
                workspace_id=ws_filter, member=request.user
            ).exists():
                return Response(
                    {"error": "forbidden"}, status=status.HTTP_403_FORBIDDEN
                )
            return Response(_serialize_projects(ws_filter))
        ws_ids = list(
            WorkspaceMember.objects.filter(member=request.user).values_list(
                "workspace_id", flat=True
            )
        )
        out: list[dict] = []
        for wid in ws_ids:
            out.extend(_serialize_projects(wid))
        return Response(out)


def _authenticated_machine(request, dev_machine_id):
    """Return the machine represented by the bearer token, if it matches."""

    token = getattr(request, "auth_machine_token", None)
    if token is None or token.dev_machine_id is None:
        return None
    if str(token.dev_machine_id) != str(dev_machine_id):
        return None
    machine = token.dev_machine
    if machine is None or machine.owner_id != token.user_id:
        return None
    return machine


def _machine_project_snapshot(machine, token):
    active_projects = list(
        projects_visible_to_machine_token(Project.objects.all(), token).order_by(
            "identifier"
        )
    )
    visible_project_ids = [project.id for project in active_projects]
    default_pods_by_project = {
        pod.project_id: pod
        for pod in Pod.objects.filter(
            project_id__in=visible_project_ids,
            is_default=True,
            deleted_at__isnull=True,
        ).order_by("project_id", "created_at")
    }
    projects = []
    for project in active_projects:
        default_pod = default_pods_by_project.get(project.id)
        projects.append(
            {
                "id": str(project.id),
                "identifier": project.identifier,
                "name": project.name,
                "default_pod_id": str(default_pod.id) if default_pod else None,
                "default_pod_slug": default_pod.name if default_pod else "",
            }
        )

    bindings = []
    for runner in (
        Runner.objects.select_related("pod__project")
        .filter(
            dev_machine_id=machine.id,
            workspace_id=token.workspace_id,
            revoked_at__isnull=True,
            pod__deleted_at__isnull=True,
            pod__project__deleted_at__isnull=True,
            pod__project_id__in=visible_project_ids,
        )
        .order_by("created_at", "id")
    ):
        project = runner.pod.project
        bindings.append(
            {
                "runner_id": str(runner.id),
                "runner_name": runner.name,
                "project_id": str(project.id),
                "project_identifier": project.identifier,
                "pod_id": str(runner.pod_id),
                "pod_slug": runner.pod.name,
                "binding_source": runner.binding_source,
            }
        )

    workspace = token.workspace
    return {
        "complete": True,
        "snapshot_id": str(uuid.uuid4()),
        "workspace_id": str(workspace.id) if workspace else None,
        "workspace_slug": workspace.slug if workspace else "",
        "projects": projects,
        "bindings": bindings,
        # Project visibility is not deletion authority: membership and
        # privacy changes can make an active project disappear from
        # ``projects`` without deleting it.  Daemons may tear down local
        # resources only for these explicit, workspace-scoped tombstones.
        "deleted_project_ids": [
            str(project_id)
            for project_id in ProjectDeletionTombstone.objects.filter(
                workspace_id=token.workspace_id,
            )
            .order_by("project_id")
            .values_list("project_id", flat=True)
        ],
    }


class MachineProjectBindingSnapshotEndpoint(APIView):
    """Return visible desired state plus explicit project tombstones."""

    authentication_classes = [MachineTokenAuthentication]
    permission_classes: list = []
    throttle_classes: list = []

    def get(self, request, dev_machine_id):
        machine = _authenticated_machine(request, dev_machine_id)
        if machine is None:
            return Response(
                {"error": "dev_machine_mismatch"},
                status=status.HTTP_403_FORBIDDEN,
            )
        return Response(_machine_project_snapshot(machine, request.auth_machine_token))
