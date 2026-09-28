# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from django.apps import AppConfig


class RunnerConfig(AppConfig):
    name = "karma_team.runner"
    label = "runner"
    verbose_name = "karma-team Runner"
    default_auto_field = "django.db.models.BigAutoField"

    def ready(self):
        # Import for side effects: registers post_save(Project) handler that
        # auto-creates a default pod per new project. See runner/signals.py.
        from karma_team.runner import signals  # noqa: F401
        from karma_team.cloud_agent import checks  # noqa: F401
        from karma_team.managed_runner import checks as managed_checks  # noqa: F401
