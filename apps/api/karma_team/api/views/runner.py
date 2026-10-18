# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""X-Api-Key authenticated runner endpoints used by the local CLI.

The web UI's session-authenticated delete already lives in
``karma_team.runner.views.runners.RunnerDetailEndpoint``. The CLI cannot
use that surface (no session cookie), so this thin wrapper exposes the
same delete via the ``/api/v1/`` MachineToken / X-Api-Key spine.

Both surfaces delegate to the same shared service
(``karma_team.runner.services.runner_delete``) so the cascade-vs-cloud-only
semantics stay in lockstep.
"""

from rest_framework import status
from rest_framework.response import Response

from karma_team.api.views.base import BaseAPIView
from karma_team.runner.models import Runner
from karma_team.runner.services.permissions import can_manage_runner, can_view_runner
from karma_team.runner.services.runner_delete import (
    delete_runner as delete_runner_svc,
    parse_purge_local,
)


class RunnerDeleteEndpoint(BaseAPIView):
    """``DELETE /api/v1/runners/<runner_id>/`` — CLI-friendly cascade delete.

    Authenticates via ``X-Api-Key`` (``APIKeyAuthentication`` from
    :mod:`karma_team.api.middleware.api_authentication`). Authorizes via
    the same ``can_manage_runner`` predicate as the web UI: the
    caller must be able to view and manage the runner. Private runners are
    owner-only.

    Accepts the same ``?purge_local=true|false`` query flag as the
    web endpoint; default is ``true`` so a CLI that omits the flag
    still gets the cascade behaviour the operator typed
    ``karma-team runner remove`` to invoke.
    """

    def delete(self, request, runner_id):
        runner = Runner.objects.filter(pk=runner_id).first()
        if runner is None:
            return Response({"error": "not found"}, status=status.HTTP_404_NOT_FOUND)
        # A machine-token caller is the project-sync daemon for exactly one
        # dev machine. Do not let a user who owns several machines use one
        # machine's credential to delete another machine's runner; the
        # reconciliation delete must remain machine-scoped just like the
        # snapshot and ensure endpoints.
        machine_token = getattr(request, "auth_machine_token", None)
        if machine_token is not None and (
            machine_token.dev_machine_id is None
            or runner.dev_machine_id != machine_token.dev_machine_id
            or runner.workspace_id != machine_token.workspace_id
        ):
            return Response({"error": "not found"}, status=status.HTTP_404_NOT_FOUND)
        if not can_view_runner(request.user, runner):
            return Response({"error": "not found"}, status=status.HTTP_404_NOT_FOUND)
        if not can_manage_runner(request.user, runner):
            return Response({"error": "forbidden"}, status=status.HTTP_403_FORBIDDEN)
        try:
            purge_local = parse_purge_local(request.query_params)
        except ValueError as exc:
            return Response({"error": str(exc)}, status=status.HTTP_400_BAD_REQUEST)
        delete_runner_svc(runner, purge_local=purge_local)
        return Response(status=status.HTTP_204_NO_CONTENT)
