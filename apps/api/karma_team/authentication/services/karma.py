"""Authentication and user synchronisation for the Karma identity service."""

from __future__ import annotations

import logging
import math
from typing import Any

import requests
from django.db import IntegrityError, transaction
from django.db.models import Q
from django.http import HttpRequest
from django.utils import timezone

from karma_team.authentication.adapter.error import (
    AUTHENTICATION_ERROR_CODES,
    AuthenticationException,
)
from karma_team.authentication.utils.user_auth_workflow import post_user_auth_workflow
from karma_team.config import get_config
from karma_team.db.models import Profile, User
from karma_team.utils.ip_address import get_client_ip

logger = logging.getLogger(__name__)

DEFAULT_KARMA_AUTH_BASE_URL = "https://karma-eos3-server.aifoundrys.com:7443"
DEFAULT_KARMA_AUTH_TIMEOUT_SECONDS = 10.0


def _authentication_failed(identifier: str) -> AuthenticationException:
    """Return the public error used for every upstream authentication failure."""

    return AuthenticationException(
        error_code=AUTHENTICATION_ERROR_CODES["AUTHENTICATION_FAILED_SIGN_IN"],
        error_message="AUTHENTICATION_FAILED_SIGN_IN",
        payload={"email": identifier},
    )


class KarmaAuthClient:
    """Small HTTP client for the Karma login and profile endpoints."""

    def __init__(self, base_url: str | None = None, timeout: float | None = None):
        configured_url = base_url or get_config("KARMA_AUTH_BASE_URL", DEFAULT_KARMA_AUTH_BASE_URL)
        self.base_url = str(configured_url or DEFAULT_KARMA_AUTH_BASE_URL).rstrip("/")
        configured_timeout = timeout
        if configured_timeout is None:
            configured_timeout = get_config("KARMA_AUTH_TIMEOUT_SECONDS", DEFAULT_KARMA_AUTH_TIMEOUT_SECONDS)
        try:
            self.timeout = float(configured_timeout)
        except (TypeError, ValueError):
            self.timeout = DEFAULT_KARMA_AUTH_TIMEOUT_SECONDS
        if not math.isfinite(self.timeout) or self.timeout <= 0:
            self.timeout = DEFAULT_KARMA_AUTH_TIMEOUT_SECONDS

    def authenticate(self, username: str, password: str) -> dict[str, Any]:
        """Authenticate credentials and return the upstream profile.

        The access token is intentionally kept within this method. It is only
        sent to the profile request and is never persisted or included in an
        exception/log message.
        """

        try:
            login_response = requests.post(
                f"{self.base_url}/v1/auth/login",
                json={"username": username, "password": password},
                timeout=self.timeout,
                allow_redirects=False,
            )
            if login_response.status_code != 200:
                raise _authentication_failed(username)
            login_payload = login_response.json()
            if not isinstance(login_payload, dict) or login_payload.get("success") is not True:
                raise _authentication_failed(username)
            token = login_payload.get("token")
            if not isinstance(token, str) or not token:
                raise _authentication_failed(username)

            profile_response = requests.get(
                f"{self.base_url}/v1/account/profile",
                headers={"Authorization": f"Bearer {token}"},
                timeout=self.timeout,
                allow_redirects=False,
            )
            if profile_response.status_code != 200:
                raise _authentication_failed(username)
            profile = profile_response.json()
            if not isinstance(profile, dict) or not isinstance(profile.get("username"), str):
                raise _authentication_failed(username)
            if not profile["username"].strip():
                raise _authentication_failed(username)
            return profile
        except AuthenticationException:
            raise
        except (requests.RequestException, ValueError, TypeError, KeyError, AttributeError):
            raise _authentication_failed(username) from None


def _profile_text(profile: dict[str, Any], key: str, field: str) -> str:
    value = profile.get(key)
    if value is None:
        return ""
    if not isinstance(value, str) or "\x00" in value:
        raise _authentication_failed("")
    value = value.strip()
    max_length = User._meta.get_field(field).max_length
    if max_length and len(value) > max_length:
        raise _authentication_failed("")
    return value


def sync_karma_user(profile: dict[str, Any], request: HttpRequest) -> User:
    """Create or overwrite the local user represented by a Karma profile."""

    if not isinstance(profile, dict):
        raise _authentication_failed("")
    username = _profile_text(profile, "username", "username")
    if not username:
        raise _authentication_failed("")
    first_name = _profile_text(profile, "firstName", "first_name")
    last_name = _profile_text(profile, "lastName", "last_name")
    display_name = " ".join(part for part in (first_name, last_name) if part) or username
    display_name = display_name[: User._meta.get_field("display_name").max_length]
    avatar = _profile_text(profile, "avatar", "avatar")

    try:
        with transaction.atomic():
            # Match legacy accounts too, but never choose arbitrarily between
            # conflicting local identities. Lock an existing row while syncing.
            users = list(
                User.objects.select_for_update().filter(Q(username__iexact=username) | Q(email__iexact=username))[:2]
            )
            if len(users) > 1:
                raise _authentication_failed(username)
            user = users[0] if users else None
            if user is None:
                user = User(username=username, email=username)
                user.set_unusable_password()
            else:
                user.username = username
                user.email = username
            user.first_name = first_name
            user.last_name = last_name
            user.display_name = display_name
            user.avatar = avatar
            # Karma's profile is authoritative. Do not let an older local
            # upload shadow the profile avatar through ``avatar_url``.
            user.avatar_asset = None
            user.is_active = True
            user.last_login_medium = "karma"
            user.last_active = timezone.now()
            user.last_login_time = timezone.now()
            user.last_login_ip = get_client_ip(request=request) or ""
            user.last_login_uagent = request.META.get("HTTP_USER_AGENT", "")
            user.token_updated_at = timezone.now()
            user.save()
            Profile.objects.get_or_create(user=user)
    except IntegrityError:
        logger.warning("Unable to synchronise Karma user due to a local identity conflict")
        raise _authentication_failed(username) from None
    return user


def authenticate_karma_user(request: HttpRequest, username: str, password: str) -> User:
    """Authenticate with Karma, sync the profile, and run common auth hooks."""

    if get_config("ENABLE_EMAIL_PASSWORD") == "0":
        raise AuthenticationException(
            error_code=AUTHENTICATION_ERROR_CODES["EMAIL_PASSWORD_AUTHENTICATION_DISABLED"],
            error_message="EMAIL_PASSWORD_AUTHENTICATION_DISABLED",
        )
    profile = KarmaAuthClient().authenticate(username=username, password=password)
    user = sync_karma_user(profile=profile, request=request)
    post_user_auth_workflow(user=user, is_signup=False, request=request)
    return user
