# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from unittest.mock import Mock, patch

import pytest
from django.http import JsonResponse
from django.test import RequestFactory

from karma_team.middleware.logger import APITokenLogMiddleware


pytestmark = pytest.mark.unit


def test_api_activity_task_redacts_sensitive_arguments_from_celery_logs():
    request = RequestFactory().get(
        "/api/v1/auth/workspaces/",
        HTTP_X_API_KEY="sensitive-machine-token",
        HTTP_USER_AGENT="karma-team-test",
    )
    request.user = Mock(is_authenticated=False)
    response = JsonResponse({"workspaces": []})
    middleware = APITokenLogMiddleware(lambda _request: response)

    with patch("karma_team.middleware.logger.process_logs") as process_logs:
        middleware.process_request(request, response, b"")

    process_logs.apply_async.assert_called_once()
    call_kwargs = process_logs.apply_async.call_args.kwargs
    assert call_kwargs["kwargsrepr"] == "<redacted>"
    assert call_kwargs["kwargs"]["log_data"]["token_identifier"] == "sensitive-machine-token"
    assert call_kwargs["kwargs"]["mongo_log"]["token_identifier"] == "sensitive-machine-token"
    assert "sensitive-machine-token" not in call_kwargs["kwargsrepr"]
