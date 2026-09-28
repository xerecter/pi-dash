# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

# Third party modules
from rest_framework import status
from rest_framework.response import Response

# Module imports
from karma_team.app.serializers import LabelSerializer
from karma_team.app.views.base import BaseAPIView
from karma_team.db.models import Label
from karma_team.app.permissions import WorkspaceViewerPermission
from karma_team.utils.cache import cache_response


class WorkspaceLabelsEndpoint(BaseAPIView):
    permission_classes = [WorkspaceViewerPermission]
    use_read_replica = True

    @cache_response(60 * 60 * 2)
    def get(self, request, slug):
        labels = Label.objects.filter(
            workspace__slug=slug,
            project__project_projectmember__member=request.user,
            project__project_projectmember__is_active=True,
            project__archived_at__isnull=True,
        )
        serializer = LabelSerializer(labels, many=True).data
        return Response(serializer, status=status.HTTP_200_OK)
