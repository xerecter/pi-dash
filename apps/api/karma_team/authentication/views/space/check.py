# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

# Python imports
import os

# Django imports
from django.db.models import Q

# Third party imports
from rest_framework import status
from rest_framework.permissions import AllowAny
from rest_framework.response import Response
from rest_framework.views import APIView

## Module imports
from karma_team.db.models import User
from karma_team.license.models import Instance
from karma_team.authentication.adapter.error import (
    AUTHENTICATION_ERROR_CODES,
    AuthenticationException,
)
from karma_team.authentication.rate_limit import AuthenticationThrottle
from karma_team.license.utils.instance_value import get_configuration_value


class EmailCheckSpaceEndpoint(APIView):
    permission_classes = [AllowAny]

    throttle_classes = [AuthenticationThrottle]

    def post(self, request):
        # Check instance configuration
        instance = Instance.objects.first()
        if instance is None or not instance.is_setup_done:
            exc = AuthenticationException(
                error_code=AUTHENTICATION_ERROR_CODES["INSTANCE_NOT_CONFIGURED"],
                error_message="INSTANCE_NOT_CONFIGURED",
            )
            return Response(exc.get_error_dict(), status=status.HTTP_400_BAD_REQUEST)

        (EMAIL_HOST, ENABLE_MAGIC_LINK_LOGIN) = get_configuration_value(
            [
                {"key": "EMAIL_HOST", "default": os.environ.get("EMAIL_HOST", "")},
                {
                    "key": "ENABLE_MAGIC_LINK_LOGIN",
                    "default": os.environ.get("ENABLE_MAGIC_LINK_LOGIN", "1"),
                },
            ]
        )

        smtp_configured = bool(EMAIL_HOST)
        is_magic_login_enabled = ENABLE_MAGIC_LINK_LOGIN == "1"

        email = str(request.data.get("email") or "").strip().lower()

        # Return error if email is not present
        if not email:
            exc = AuthenticationException(
                error_code=AUTHENTICATION_ERROR_CODES["EMAIL_REQUIRED"],
                error_message="EMAIL_REQUIRED",
            )
            return Response(exc.get_error_dict(), status=status.HTTP_400_BAD_REQUEST)

        # ``email`` is the legacy request key. Accept either a Karma username
        # or an email-shaped account identifier.
        existing_user = User.objects.filter(Q(email__iexact=email) | Q(username__iexact=email)).first()

        # If existing user
        if existing_user:
            # Return response
            return Response(
                {
                    "existing": True,
                    "status": (
                        "MAGIC_CODE"
                        if existing_user.is_password_autoset and smtp_configured and is_magic_login_enabled
                        else "CREDENTIAL"
                    ),
                },
                status=status.HTTP_200_OK,
            )
        # Else return response
        return Response(
            {
                "existing": False,
                "status": ("MAGIC_CODE" if smtp_configured and is_magic_login_enabled else "CREDENTIAL"),
            },
            status=status.HTTP_200_OK,
        )
