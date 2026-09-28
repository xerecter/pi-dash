# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from rest_framework import status
from rest_framework.response import Response

from karma_team.app.permissions import ROLE, allow_permission
from karma_team.app.serializers import IssueSerializer
from karma_team.db.models import Issue
from karma_team.utils.host import base_host
from karma_team.utils.issue_move import IssueMoveError, move_work_item_to_project

from .. import BaseAPIView


class IssueMoveEndpoint(BaseAPIView):
    """Session-authed counterpart to ``IssueMoveAPIEndpoint``.

    Powers the web app's "Move to project" action. The heavy lifting lives in
    ``karma_team.utils.issue_move.move_work_item_to_project`` so this and the
    API-key endpoint stay in lockstep.
    """

    model = Issue
    webhook_event = "issue"

    @allow_permission([ROLE.ADMIN, ROLE.MEMBER])
    def post(self, request, slug, project_id, pk):
        # A non-dict body (JSON list/primitive) has no ``.get`` — coerce it so a
        # malformed payload becomes a clean 400 ("project is required") rather
        # than an AttributeError 500.
        data = request.data if isinstance(request.data, dict) else {}
        try:
            issue = move_work_item_to_project(
                slug=slug,
                project_id=project_id,
                pk=pk,
                target_ref=data.get("project"),
                actor=request.user,
                origin=base_host(request=request, is_app=True),
            )
        except IssueMoveError as exc:
            return Response({"error": exc.message}, status=exc.status_code)

        return Response(IssueSerializer(issue).data, status=status.HTTP_200_OK)
