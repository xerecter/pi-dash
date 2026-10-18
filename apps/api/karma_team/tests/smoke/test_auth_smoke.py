# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import pytest
import requests
import uuid
from unittest.mock import patch
from django.urls import reverse
from django.utils import timezone

from karma_team.authentication.adapter.error import AUTHENTICATION_ERROR_CODES, AuthenticationException
from karma_team.db.models import User
from karma_team.license.models import Instance


@pytest.mark.smoke
class TestAuthSmoke:
    """Smoke tests for authentication endpoints"""

    @pytest.mark.django_db(transaction=True)
    def test_login_endpoint_available(self, karma_team_server, settings):
        """Exercise the real HTTP and session flow without a live Karma dependency."""
        Instance.objects.create(
            instance_name="Auth Smoke Test",
            instance_id=str(uuid.uuid4()),
            current_version="1.0.0",
            domain=karma_team_server.url,
            last_checked_at=timezone.now(),
            is_setup_done=True,
        )
        url = f"{karma_team_server.url}{reverse('sign-in')}"
        profile = {"id": "EOS3-smoke-user", "username": "smoke-user", "firstName": "Smoke", "lastName": "Test"}
        failure = AuthenticationException(
            error_code=AUTHENTICATION_ERROR_CODES["AUTHENTICATION_FAILED_SIGN_IN"],
            error_message="AUTHENTICATION_FAILED_SIGN_IN",
        )

        with (
            requests.Session() as session,
            patch(
                "karma_team.authentication.services.karma.KarmaAuthClient.authenticate",
                side_effect=[failure, profile],
            ) as karma_authenticate,
        ):
            csrf = session.get(f"{karma_team_server.url}{reverse('get_csrf_token')}", timeout=10)
            assert csrf.status_code == 200
            headers = {"X-CSRFToken": csrf.json()["csrf_token"]}

            response = session.post(
                url,
                data={"email": "smoke-user", "password": "wrong-password"},
                headers=headers,
                allow_redirects=False,
                timeout=10,
            )
            assert response.status_code == 302
            assert "AUTHENTICATION_FAILED_SIGN_IN" in response.headers["Location"]
            assert not User.objects.filter(username="smoke-user").exists()
            assert settings.SESSION_COOKIE_NAME not in session.cookies

            response = session.post(
                url,
                data={"email": "smoke-user", "password": "karma-password"},
                headers=headers,
                allow_redirects=False,
                timeout=10,
            )
            assert response.status_code == 302
            assert "error_code" not in response.headers["Location"]
            assert settings.SESSION_COOKIE_NAME in session.cookies
            assert User.objects.get(username="smoke-user").email == "smoke-user"
            assert karma_authenticate.call_count == 2


@pytest.mark.smoke
class TestHealthCheckSmoke:
    """Smoke test for health check endpoint"""

    def test_healthcheck_endpoint(self, karma_team_server):
        """Test that the health check endpoint is available and responds correctly"""
        # Make a request to the health check endpoint
        response = requests.get(f"{karma_team_server.url}/")

        # Should be OK
        assert response.status_code == 200, "Health check endpoint should return 200 OK"
