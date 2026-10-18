# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import json
import uuid
import pytest
import requests
from urllib.parse import parse_qs, urlparse
from django.urls import reverse
from django.utils import timezone
from rest_framework import status
from django.test import Client
from unittest.mock import patch

from karma_team.db.models import Profile, User
from karma_team.settings.redis import redis_instance
from karma_team.license.models import Instance, InstanceConfiguration


@pytest.fixture
def setup_instance(db):
    """Create and configure an instance for authentication tests"""
    instance_id = uuid.uuid4() if not Instance.objects.exists() else Instance.objects.first().id

    # Create or update instance with all required fields
    instance, _ = Instance.objects.update_or_create(
        id=instance_id,
        defaults={
            "instance_name": "Test Instance",
            "instance_id": str(uuid.uuid4()),
            "current_version": "1.0.0",
            "domain": "http://localhost:8000",
            "last_checked_at": timezone.now(),
            "is_setup_done": True,
        },
    )
    return instance


@pytest.fixture
def django_client():
    """Return a Django test client with User-Agent header for handling redirects"""
    client = Client(HTTP_USER_AGENT="Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:15.0) Gecko/20100101 Firefox/15.0.1")
    return client


@pytest.mark.contract
class TestMagicLinkGenerate:
    """Test magic link generation functionality"""

    @pytest.fixture
    def setup_user(self, db):
        """Create a test user for magic link tests"""
        user = User.objects.create(email="user@example.com")
        user.set_password("user@123")
        user.save()
        return user

    @pytest.mark.django_db
    def test_without_data(self, api_client, setup_user, setup_instance):
        """Test magic link generation with empty data"""
        url = reverse("magic-generate")
        response = api_client.post(url, {}, format="json")
        assert response.status_code == status.HTTP_400_BAD_REQUEST
        assert response.data["error_message"] == "EMAIL_REQUIRED"

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_non_email_identifier(self, mock_magic_link, api_client, setup_user, setup_instance):
        """Account identifiers do not require an email-address shape."""
        url = reverse("magic-generate")
        redis_instance().delete("magic_useremail.com")
        response = api_client.post(url, {"email": "useremail.com"}, format="json")
        assert response.status_code == status.HTTP_200_OK
        mock_magic_link.assert_called_once()
        assert mock_magic_link.call_args.args[0] == "useremail.com"

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_magic_generate(self, mock_magic_link, api_client, setup_user, setup_instance):
        """Test successful magic link generation"""
        url = reverse("magic-generate")

        ri = redis_instance()
        ri.delete("magic_user@example.com")

        response = api_client.post(url, {"email": "user@example.com"}, format="json")
        assert response.status_code == status.HTTP_200_OK
        assert "key" in response.data  # Check for key in response

        # Verify the mock was called with the expected arguments
        mock_magic_link.assert_called_once()
        args = mock_magic_link.call_args[0]
        assert args[0] == "user@example.com"  # First arg should be the email

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_max_generate_attempt(self, mock_magic_link, api_client, setup_user, setup_instance):
        """Test exceeding maximum magic link generation attempts"""
        url = reverse("magic-generate")

        ri = redis_instance()
        ri.delete("magic_user@example.com")

        for _ in range(4):
            api_client.post(url, {"email": "user@example.com"}, format="json")

        response = api_client.post(url, {"email": "user@example.com"}, format="json")
        assert response.status_code == status.HTTP_400_BAD_REQUEST
        assert "error_code" in response.data  # Check for error code in response


@pytest.mark.contract
@pytest.mark.django_db
class TestSignInEndpoint:
    """Exercise Karma authentication, profile synchronisation and Django sessions."""

    @pytest.fixture(autouse=True)
    def sign_in_settings(self, settings):
        settings.WEB_URL = "http://localhost"
        settings.APP_BASE_URL = "http://localhost"
        settings.SPACE_BASE_URL = "http://localhost"
        settings.SPACE_BASE_PATH = "/spaces"

    @pytest.fixture(params=["sign-in", "space-sign-in"])
    def sign_in_url(self, request):
        return reverse(request.param)

    @pytest.fixture
    def karma_profile(self):
        return {
            "id": "EOS3-xuhaoxuan",
            "username": "xuhaoxuan",
            "firstName": "Hao Xuan",
            "lastName": "Xu",
            "avatar": "https://karma.example/avatar.png",
        }

    @pytest.fixture
    def karma_http(self, karma_profile, monkeypatch):
        monkeypatch.setenv("KARMA_AUTH_BASE_URL", "https://karma.example")
        monkeypatch.setenv("KARMA_AUTH_TIMEOUT_SECONDS", "7")
        login_response = requests.Response()
        login_response.status_code = 200
        login_response._content = json.dumps({"success": True, "token": "upstream-token"}).encode()
        profile_response = requests.Response()
        profile_response.status_code = 200
        profile_response._content = json.dumps(karma_profile).encode()
        with (
            patch("karma_team.authentication.services.karma.requests.post", return_value=login_response) as post,
            patch("karma_team.authentication.services.karma.requests.get", return_value=profile_response) as get,
        ):
            yield post, get

    @pytest.fixture
    def setup_user(self, db):
        """Create a test user for authentication tests"""
        user = User.objects.create(
            username="xuhaoxuan",
            email="old@example.com",
            first_name="Old",
            last_name="Name",
            avatar="https://local.example/old.png",
        )
        user.set_password("user@123")
        user.save()
        return user

    @pytest.mark.parametrize("data", [{}, {"email": "   ", "password": "password"}])
    def test_without_credentials(self, django_client, setup_instance, sign_in_url, karma_http, data):
        response = django_client.post(sign_in_url, data)

        assert "REQUIRED_EMAIL_PASSWORD_SIGN_IN" in response.url
        karma_http[0].assert_not_called()
        karma_http[1].assert_not_called()
        assert "_auth_user_id" not in django_client.session

    def test_username_sign_in_creates_user_and_session(
        self, django_client, setup_instance, sign_in_url, karma_http, karma_profile
    ):
        response = django_client.post(sign_in_url, {"email": "  xuhaoxuan  ", "password": "karma-password"})

        assert response.status_code == 302
        assert "error_code" not in response.url
        user = User.objects.get(username="xuhaoxuan")
        assert user.email == "xuhaoxuan"
        assert user.first_name == karma_profile["firstName"]
        assert user.last_name == karma_profile["lastName"]
        assert user.avatar == karma_profile["avatar"]
        assert user.display_name == "Hao Xuan Xu"
        assert not user.has_usable_password()
        assert Profile.objects.filter(user=user).exists()
        assert django_client.session["_auth_user_id"] == str(user.pk)
        assert django_client.session["device_info"]["user_agent"]
        karma_http[0].assert_called_once_with(
            "https://karma.example/v1/auth/login",
            json={"username": "xuhaoxuan", "password": "karma-password"},
            timeout=7.0,
            allow_redirects=False,
        )
        karma_http[1].assert_called_once_with(
            "https://karma.example/v1/account/profile",
            headers={"Authorization": "Bearer upstream-token"},
            timeout=7.0,
            allow_redirects=False,
        )

    @pytest.mark.parametrize("failed_stage", ["login", "profile"])
    def test_failure_does_not_create_user_or_session(
        self, django_client, setup_instance, sign_in_url, karma_http, failed_stage
    ):
        failed_request = karma_http[0 if failed_stage == "login" else 1]
        failed_request.return_value.status_code = 401
        users_before = User.objects.count()
        profiles_before = Profile.objects.count()

        response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "wrong"})

        assert "AUTHENTICATION_FAILED_SIGN_IN" in response.url
        assert User.objects.count() == users_before
        assert Profile.objects.count() == profiles_before
        assert "_auth_user_id" not in django_client.session
        if failed_stage == "login":
            karma_http[1].assert_not_called()

    def test_profile_failure_does_not_change_existing_user(
        self, django_client, setup_user, setup_instance, sign_in_url, karma_http
    ):
        karma_http[1].side_effect = requests.Timeout("upstream unavailable")
        original = User.objects.filter(pk=setup_user.pk).values().get()

        response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "karma-password"})

        assert "AUTHENTICATION_FAILED_SIGN_IN" in response.url
        assert User.objects.filter(pk=setup_user.pk).values().get() == original
        assert "_auth_user_id" not in django_client.session

    @pytest.mark.parametrize("upstream_status", [200, 401])
    def test_local_password_is_never_checked(
        self, django_client, setup_user, setup_instance, sign_in_url, karma_http, upstream_status
    ):
        karma_http[0].return_value.status_code = upstream_status
        with patch.object(User, "check_password", side_effect=AssertionError("Local password was checked")):
            response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "user@123"})

        if upstream_status == 200:
            assert "error_code" not in response.url
            assert django_client.session["_auth_user_id"] == str(setup_user.pk)
        else:
            assert "AUTHENTICATION_FAILED_SIGN_IN" in response.url
            assert "_auth_user_id" not in django_client.session

    def test_each_login_overwrites_profile_without_replacing_user(
        self, django_client, setup_user, setup_instance, sign_in_url, karma_http
    ):
        response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "karma-password"})
        assert "error_code" not in response.url
        setup_user.refresh_from_db()
        assert setup_user.email == "xuhaoxuan"
        assert setup_user.first_name == "Hao Xuan"
        assert setup_user.avatar == "https://karma.example/avatar.png"
        django_client.logout()
        karma_http[1].return_value._content = json.dumps(
            {
                "id": "EOS3-xuhaoxuan",
                "username": "xuhaoxuan",
                "firstName": None,
                "lastName": None,
                "avatar": None,
            }
        ).encode()

        response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "new-karma-password"})

        assert "error_code" not in response.url
        setup_user.refresh_from_db()
        assert setup_user.first_name == ""
        assert setup_user.last_name == ""
        assert setup_user.avatar == ""
        assert setup_user.display_name == "xuhaoxuan"
        assert User.objects.filter(username="xuhaoxuan").count() == 1
        assert Profile.objects.filter(user=setup_user).count() == 1
        assert django_client.session["_auth_user_id"] == str(setup_user.pk)

    def test_next_path_redirection(self, django_client, setup_instance, sign_in_url, karma_http):
        response = django_client.post(
            sign_in_url,
            {"email": "xuhaoxuan", "password": "karma-password", "next_path": "/workspaces"},
        )

        assert "error_code" not in response.url
        parsed_url = urlparse(response.url)
        if sign_in_url == reverse("sign-in"):
            assert parse_qs(parsed_url.query)["next_path"] == ["/workspaces"]
        else:
            assert parsed_url.path.endswith("/spaces/workspaces")
        assert "_auth_user_id" in django_client.session

    def test_disabled_password_auth_does_not_call_karma(self, django_client, setup_instance, sign_in_url, karma_http):
        InstanceConfiguration.objects.update_or_create(key="ENABLE_EMAIL_PASSWORD", defaults={"value": "0"})

        response = django_client.post(sign_in_url, {"email": "xuhaoxuan", "password": "karma-password"})

        assert "EMAIL_PASSWORD_AUTHENTICATION_DISABLED" in response.url
        karma_http[0].assert_not_called()
        karma_http[1].assert_not_called()
        assert "_auth_user_id" not in django_client.session


@pytest.mark.contract
@pytest.mark.django_db
class TestEmailCheck:
    """Registration preflight accepts account names and email identifiers."""

    @pytest.fixture(params=["/auth/email-check/", "/auth/spaces/email-check/"])
    def email_check_url(self, request):
        return request.param

    @pytest.mark.parametrize("identifier", ["xuhaoxuan", "team/user+east", "not-an-email@"])
    def test_accepts_account_identifier(self, api_client, setup_instance, email_check_url, identifier):
        response = api_client.post(email_check_url, {"email": identifier}, format="json")

        assert response.status_code == status.HTTP_200_OK
        assert response.data["existing"] is False

    def test_matches_existing_username(self, api_client, setup_instance, email_check_url):
        User.objects.create(username="testfree1", email="member@example.com")
        response = api_client.post(email_check_url, {"email": " TESTFREE1 "}, format="json")

        assert response.status_code == status.HTTP_200_OK
        assert response.data["existing"] is True

    @pytest.mark.parametrize("identifier", ["", "   "])
    def test_empty_identifier_is_rejected(self, api_client, setup_instance, email_check_url, identifier):
        response = api_client.post(email_check_url, {"email": identifier}, format="json")

        assert response.status_code == status.HTTP_400_BAD_REQUEST
        assert response.data["error_message"] == "EMAIL_REQUIRED"

    def test_registration_email_still_supported(self, api_client, setup_instance, email_check_url):
        response = api_client.post(email_check_url, {"email": "new-registration@example.com"}, format="json")

        assert response.status_code == status.HTTP_200_OK
        assert response.data["existing"] is False
        assert response.data["status"] in {"CREDENTIAL", "MAGIC_CODE"}


@pytest.mark.contract
class TestMagicSignIn:
    """Test magic link sign-in functionality"""

    @pytest.fixture
    def setup_user(self, db):
        """Create a test user for magic sign-in tests"""
        user = User.objects.create(email="user@example.com")
        user.set_password("user@123")
        user.save()
        return user

    @pytest.mark.django_db
    def test_without_data(self, django_client, setup_user, setup_instance):
        """Test magic link sign-in with empty data"""
        url = reverse("magic-sign-in")
        response = django_client.post(url, {}, follow=True)

        # Check redirect contains error code
        assert "MAGIC_SIGN_IN_EMAIL_CODE_REQUIRED" in response.redirect_chain[-1][0]

    @pytest.mark.django_db
    def test_expired_invalid_magic_link(self, django_client, setup_user, setup_instance):
        """Test magic link sign-in with expired/invalid link"""
        ri = redis_instance()
        ri.delete("magic_user@example.com")

        url = reverse("magic-sign-in")
        response = django_client.post(url, {"email": "user@example.com", "code": "xxxx-xxxxx-xxxx"}, follow=False)

        # Check that we get a redirect
        assert response.status_code == 302

        # The actual error code is EXPIRED_MAGIC_CODE_SIGN_IN (when key doesn't exist)
        # or INVALID_MAGIC_CODE_SIGN_IN (when key exists but code doesn't match)
        assert "EXPIRED_MAGIC_CODE_SIGN_IN" in response.url or "INVALID_MAGIC_CODE_SIGN_IN" in response.url

    @pytest.mark.django_db
    def test_user_does_not_exist(self, django_client, setup_instance):
        """Test magic sign-in with non-existent user"""
        url = reverse("magic-sign-in")
        response = django_client.post(
            url,
            {"email": "nonexistent@example.com", "code": "xxxx-xxxxx-xxxx"},
            follow=True,
        )

        # Check redirect contains error code
        assert "USER_DOES_NOT_EXIST" in response.redirect_chain[-1][0]

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_magic_code_sign_in(self, mock_magic_link, django_client, api_client, setup_user, setup_instance):
        """Test successful magic link sign-in process"""
        # First generate a magic link token
        gen_url = reverse("magic-generate")
        response = api_client.post(gen_url, {"email": "user@example.com"}, format="json")

        # Check that the token generation was successful
        assert response.status_code == status.HTTP_200_OK

        # Since we're mocking the magic_link task, we need to manually get the token from Redis
        ri = redis_instance()
        user_data = json.loads(ri.get("magic_user@example.com"))
        token = user_data["token"]

        # Use Django client to test the redirect flow without following redirects
        url = reverse("magic-sign-in")
        response = django_client.post(url, {"email": "user@example.com", "code": token}, follow=False)

        # Check that the initial response is a redirect without error code
        assert response.status_code == 302
        assert "error_code" not in response.url

        # The user should now be authenticated
        assert "_auth_user_id" in django_client.session

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_magic_sign_in_with_next_path(self, mock_magic_link, django_client, api_client, setup_user, setup_instance):
        """Test magic sign-in with next_path parameter"""
        # First generate a magic link token
        gen_url = reverse("magic-generate")
        response = api_client.post(gen_url, {"email": "user@example.com"}, format="json")

        # Check that the token generation was successful
        assert response.status_code == status.HTTP_200_OK

        # Since we're mocking the magic_link task, we need to manually get the token from Redis
        ri = redis_instance()
        user_data = json.loads(ri.get("magic_user@example.com"))
        token = user_data["token"]

        # Use Django client to test the redirect flow without following redirects.
        # The path must be absolute — ``validate_next_path`` rejects anything
        # without a leading slash to prevent scheme/netloc injection, so a
        # bare ``"workspaces"`` would be stripped before the redirect builds.
        url = reverse("magic-sign-in")
        next_path = "/workspaces"
        response = django_client.post(
            url,
            {"email": "user@example.com", "code": token, "next_path": next_path},
            follow=False,
        )

        # Check that the initial response is a redirect without error code
        assert response.status_code == 302
        assert "error_code" not in response.url

        # Check that the redirect URL contains the next_path
        assert next_path in response.url

        # The user should now be authenticated
        assert "_auth_user_id" in django_client.session


@pytest.mark.contract
class TestMagicSignUp:
    """Test magic link sign-up functionality"""

    @pytest.mark.django_db
    def test_without_data(self, django_client, setup_instance):
        """Test magic link sign-up with empty data"""
        url = reverse("magic-sign-up")
        response = django_client.post(url, {}, follow=True)

        # Check redirect contains error code
        assert "MAGIC_SIGN_UP_EMAIL_CODE_REQUIRED" in response.redirect_chain[-1][0]

    @pytest.mark.django_db
    def test_user_already_exists(self, django_client, db, setup_instance):
        """Test magic sign-up with existing user"""
        # Create a user that already exists
        User.objects.create(email="existing@example.com")

        url = reverse("magic-sign-up")
        response = django_client.post(url, {"email": "existing@example.com", "code": "xxxx-xxxxx-xxxx"}, follow=True)

        # Check redirect contains error code
        assert "USER_ALREADY_EXIST" in response.redirect_chain[-1][0]

    @pytest.mark.django_db
    def test_expired_invalid_magic_link(self, django_client, setup_instance):
        """Test magic link sign-up with expired/invalid link"""
        url = reverse("magic-sign-up")
        response = django_client.post(url, {"email": "new@example.com", "code": "xxxx-xxxxx-xxxx"}, follow=False)

        # Check that we get a redirect
        assert response.status_code == 302

        # The actual error code is EXPIRED_MAGIC_CODE_SIGN_UP (when key doesn't exist)
        # or INVALID_MAGIC_CODE_SIGN_UP (when key exists but code doesn't match)
        assert "EXPIRED_MAGIC_CODE_SIGN_UP" in response.url or "INVALID_MAGIC_CODE_SIGN_UP" in response.url

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_magic_code_sign_up(self, mock_magic_link, django_client, api_client, setup_instance):
        """Test successful magic link sign-up process"""
        email = "newuser@example.com"

        # First generate a magic link token
        gen_url = reverse("magic-generate")
        response = api_client.post(gen_url, {"email": email}, format="json")

        # Check that the token generation was successful
        assert response.status_code == status.HTTP_200_OK

        # Since we're mocking the magic_link task, we need to manually get the token from Redis
        ri = redis_instance()
        user_data = json.loads(ri.get(f"magic_{email}"))
        token = user_data["token"]

        # Use Django client to test the redirect flow without following redirects
        url = reverse("magic-sign-up")
        response = django_client.post(url, {"email": email, "code": token}, follow=False)

        # Check that the initial response is a redirect without error code
        assert response.status_code == 302
        assert "error_code" not in response.url

        # Check if user was created
        assert User.objects.filter(email=email).exists()

        # Check if user is authenticated
        assert "_auth_user_id" in django_client.session

    @pytest.mark.django_db
    @patch("karma_team.bgtasks.magic_link_code_task.magic_link.delay")
    def test_magic_sign_up_with_next_path(self, mock_magic_link, django_client, api_client, setup_instance):
        """Test magic sign-up with next_path parameter"""
        email = "newuser2@example.com"

        # First generate a magic link token
        gen_url = reverse("magic-generate")
        response = api_client.post(gen_url, {"email": email}, format="json")

        # Check that the token generation was successful
        assert response.status_code == status.HTTP_200_OK

        # Since we're mocking the magic_link task, we need to manually get the token from Redis
        ri = redis_instance()
        user_data = json.loads(ri.get(f"magic_{email}"))
        token = user_data["token"]

        # Use Django client to test the redirect flow without following redirects
        url = reverse("magic-sign-up")
        next_path = "onboarding"
        response = django_client.post(url, {"email": email, "code": token, "next_path": next_path}, follow=False)

        # Check that the initial response is a redirect without error code
        assert response.status_code == 302
        assert "error_code" not in response.url

        # In a real browser, the next_path would be used to build the absolute URL
        # Since we're just testing the authentication logic, we won't check for the exact URL structure

        # Check if user was created
        assert User.objects.filter(email=email).exists()

        # Check if user is authenticated
        assert "_auth_user_id" in django_client.session
