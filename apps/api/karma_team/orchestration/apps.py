# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from django.apps import AppConfig


class OrchestrationConfig(AppConfig):
    name = "karma_team.orchestration"
    label = "orchestration"
    verbose_name = "karma-team Orchestration"
    default_auto_field = "django.db.models.BigAutoField"

    def ready(self) -> None:
        # Importing the signals module wires up the state-transition hook.
        from karma_team.orchestration import signals  # noqa: F401
