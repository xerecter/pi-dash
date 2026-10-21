# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from django.urls import path

from karma_team.runner.views.cli_downloads import (
    cli_binary,
    cli_install_config,
    cli_installer,
    cli_uninstaller,
    cli_windows_msi,
)

app_name = "cli"

urlpatterns = [
    path("install-config/", cli_install_config, name="install-config"),
    path("install.sh", cli_installer, {"filename": "install.sh"}, name="install-sh"),
    path("install.ps1", cli_installer, {"filename": "install.ps1"}, name="install-ps1"),
    path("update.sh", cli_installer, {"filename": "update.sh"}, name="update-sh"),
    path("uninstall.sh", cli_uninstaller, name="uninstall-sh"),
    path("bin/<str:target>/", cli_binary, name="binary"),
    path("windows.msi", cli_windows_msi, name="windows-msi"),
]
