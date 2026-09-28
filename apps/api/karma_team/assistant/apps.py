# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from django.apps import AppConfig


class AssistantConfig(AppConfig):
    default_auto_field = "django.db.models.BigAutoField"
    name = "karma_team.assistant"
    label = "assistant"
    verbose_name = "AI Assistant"

    def ready(self):
        # Register tools onto the module-level agent at import time.
        from karma_team.assistant import tools  # noqa: F401
