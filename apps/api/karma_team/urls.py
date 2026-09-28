# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""karma-team URL Configuration"""

from django.conf import settings
from django.urls import include, path, re_path
from drf_spectacular.views import (
    SpectacularAPIView,
    SpectacularRedocView,
    SpectacularSwaggerView,
)

handler404 = "karma_team.app.views.error_404.custom_404_view"

urlpatterns = [
    path("api/", include("karma_team.app.urls")),
    path("api/", include("karma_team.assistant.urls")),
    path("api/", include("karma_team.loop.urls")),
    path("api/public/", include("karma_team.space.urls")),
    path("api/instances/", include("karma_team.license.urls")),
    path("api/runners/", include("karma_team.runner.web_urls")),
    path("api/v1/", include("karma_team.api.urls")),
    path("api/", include("karma_team.prompting.urls")),
    path("api/v1/runner/", include("karma_team.runner.urls")),
    path("auth/", include("karma_team.authentication.urls")),
    path("", include("karma_team.web.urls")),
]

if settings.ENABLE_DRF_SPECTACULAR:
    urlpatterns += [
        path("api/schema/", SpectacularAPIView.as_view(), name="schema"),
        path(
            "api/schema/swagger-ui/",
            SpectacularSwaggerView.as_view(url_name="schema"),
            name="swagger-ui",
        ),
        path(
            "api/schema/redoc/",
            SpectacularRedocView.as_view(url_name="schema"),
            name="redoc",
        ),
    ]

if settings.DEBUG:
    try:
        import debug_toolbar

        urlpatterns = [re_path(r"^__debug__/", include(debug_toolbar.urls))] + urlpatterns
    except ImportError:
        pass
