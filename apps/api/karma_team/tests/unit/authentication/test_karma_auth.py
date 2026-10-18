"""Tests for authentication against the external Karma identity service."""

from unittest.mock import Mock, patch

import pytest
from django.test import RequestFactory

from karma_team.authentication.adapter.error import (
    AUTHENTICATION_ERROR_CODES,
    AuthenticationException,
)
from karma_team.authentication.services.karma import KarmaAuthClient, sync_karma_user
from karma_team.db.models import Profile, User


@pytest.mark.unit
def test_karma_client_authenticates_and_fetches_profile(monkeypatch):
    monkeypatch.setenv("KARMA_AUTH_BASE_URL", "https://karma.example.test")
    post_response = Mock(status_code=200)
    post_response.json.return_value = {"success": True, "token": "token-123"}
    profile_response = Mock(status_code=200)
    profile_response.json.return_value = {
        "id": "external-1",
        "username": "alice",
        "firstName": "Alice",
        "lastName": "Karma",
    }

    with patch("karma_team.authentication.services.karma.requests.post", return_value=post_response) as post:
        with patch("karma_team.authentication.services.karma.requests.get", return_value=profile_response) as get:
            profile = KarmaAuthClient().authenticate("alice", "secret")

    post.assert_called_once_with(
        "https://karma.example.test/v1/auth/login",
        json={"username": "alice", "password": "secret"},
        timeout=10,
        allow_redirects=False,
    )
    get.assert_called_once_with(
        "https://karma.example.test/v1/account/profile",
        headers={"Authorization": "Bearer token-123"},
        timeout=10,
        allow_redirects=False,
    )
    assert profile["username"] == "alice"


@pytest.mark.unit
def test_karma_client_maps_invalid_credentials_to_authentication_error(monkeypatch):
    monkeypatch.setenv("KARMA_AUTH_BASE_URL", "https://karma.example.test")
    response = Mock(status_code=401)
    response.json.return_value = {"error": "Invalid username or password"}

    with patch("karma_team.authentication.services.karma.requests.post", return_value=response):
        with pytest.raises(AuthenticationException) as exc_info:
            KarmaAuthClient().authenticate("alice", "wrong")

    assert exc_info.value.error_code == AUTHENTICATION_ERROR_CODES["AUTHENTICATION_FAILED_SIGN_IN"]


@pytest.mark.unit
@pytest.mark.django_db
def test_sync_karma_user_creates_profile_and_allows_non_email_username():
    request = RequestFactory().post("/auth/sign-in/")
    request.META["REMOTE_ADDR"] = "127.0.0.1"
    profile = {
        "id": "external-1",
        "username": "xuhaoxuan",
        "firstName": "Xu",
        "lastName": "HaoXuan",
        "avatar": "https://example.test/avatar.png",
    }

    user = sync_karma_user(profile, request)

    assert user.username == "xuhaoxuan"
    assert user.email == "xuhaoxuan"
    assert user.first_name == "Xu"
    assert user.last_name == "HaoXuan"
    assert user.display_name == "Xu HaoXuan"
    assert Profile.objects.filter(user=user).exists()


@pytest.mark.unit
@pytest.mark.django_db
def test_sync_karma_user_overwrites_existing_profile_data():
    user = User.objects.create(
        username="xuhaoxuan",
        email="old@example.test",
        first_name="Old",
        last_name="Name",
        display_name="Old Name",
        avatar="old-avatar",
    )
    request = RequestFactory().post("/auth/sign-in/")
    profile = {
        "username": "xuhaoxuan",
        "firstName": "New",
        "lastName": "Person",
        "avatar": "new-avatar",
    }

    updated = sync_karma_user(profile, request)

    assert updated.pk == user.pk
    assert updated.email == "xuhaoxuan"
    assert updated.first_name == "New"
    assert updated.last_name == "Person"
    assert updated.display_name == "New Person"
    assert updated.avatar == "new-avatar"


@pytest.mark.unit
@pytest.mark.django_db
def test_sync_karma_user_rejects_profile_without_username():
    request = RequestFactory().post("/auth/sign-in/")

    with pytest.raises(AuthenticationException):
        sync_karma_user({}, request)


@pytest.mark.unit
@pytest.mark.parametrize("timeout", [0, -1, float("inf"), float("nan"), "invalid"])
def test_karma_client_invalid_timeout_falls_back_to_bounded_default(timeout):
    assert KarmaAuthClient(base_url="https://karma.example.test", timeout=timeout).timeout == 10


@pytest.mark.unit
@pytest.mark.django_db
@pytest.mark.parametrize(
    "invalid_field",
    [
        {"username": "a" * 129},
        {"username": "bad\x00account"},
        {"firstName": "a" * 256},
        {"lastName": "a" * 256},
        {"firstName": {"unexpected": "object"}},
        {"lastName": ["unexpected"]},
        {"avatar": {"unexpected": "object"}},
        {"firstName": False},
    ],
)
def test_invalid_profile_is_rejected_without_changing_user(invalid_field):
    user = User.objects.create(username="alice", email="alice", first_name="Original")
    original = User.objects.filter(pk=user.pk).values().get()
    request = RequestFactory().post("/auth/sign-in/")

    with pytest.raises(AuthenticationException):
        sync_karma_user({"username": "alice", **invalid_field}, request)

    assert User.objects.filter(pk=user.pk).values().get() == original
    assert User.objects.count() == 1
    assert not Profile.objects.filter(user=user).exists()


@pytest.mark.unit
@pytest.mark.django_db
def test_combined_display_name_is_bounded_without_truncating_source_names():
    request = RequestFactory().post("/auth/sign-in/")
    user = sync_karma_user({"username": "alice", "firstName": "a" * 255, "lastName": "b" * 255}, request)

    assert user.first_name == "a" * 255
    assert user.last_name == "b" * 255
    assert len(user.display_name) == 255


@pytest.mark.unit
@pytest.mark.django_db
def test_ambiguous_local_identity_is_rejected_without_overwriting_either_user():
    first = User.objects.create(username="Alice", email="first@example.test", first_name="First")
    second = User.objects.create(username="alice", email="second@example.test", first_name="Second")
    request = RequestFactory().post("/auth/sign-in/")

    with pytest.raises(AuthenticationException):
        sync_karma_user({"username": "alice", "firstName": "New"}, request)

    first.refresh_from_db()
    second.refresh_from_db()
    assert first.first_name == "First"
    assert second.first_name == "Second"


@pytest.mark.unit
@pytest.mark.django_db
def test_sync_preserves_local_permissions_and_does_not_verify_email():
    user = User.objects.create(username="alice", email="old@example.test", is_staff=True, is_email_verified=False)
    request = RequestFactory().post("/auth/sign-in/")
    updated = sync_karma_user({"username": "alice", "labAdmin": True}, request)

    assert updated.pk == user.pk
    assert updated.is_staff
    assert not updated.is_superuser
    assert not updated.is_email_verified
