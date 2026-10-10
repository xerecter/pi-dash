# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import os
import subprocess

import pytest
from django.test import override_settings

from karma_team.runner.views.cli_downloads import CLI_BINARIES, WINDOWS_MSI

pytestmark = pytest.mark.unit


@pytest.fixture(autouse=True)
def cli_settings(tmp_path):
    artifacts = tmp_path / "artifacts"
    installers = tmp_path / "installers"
    artifacts.mkdir()
    installers.mkdir()
    with override_settings(
        KARMA_TEAM_PUBLIC_URL="http://172.16.13.81",
        KARMA_TEAM_CLI_ARTIFACT_DIR=str(artifacts),
        KARMA_TEAM_CLI_INSTALLER_DIR=str(installers),
        MIDDLEWARE=[],
    ):
        yield artifacts, installers


def _publish(artifacts, target, content=b"test-cli-binary"):
    destination = artifacts / target / CLI_BINARIES[target]
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(content)
    return destination


def test_install_config_is_public_and_lists_only_published_targets(api_client, cli_settings):
    artifacts, _ = cli_settings
    _publish(artifacts, "x86_64-unknown-linux-gnu")

    response = api_client.get("/api/cli/install-config/")

    assert response.status_code == 200
    assert response.json() == {
        "service_url": "http://172.16.13.81",
        "install_sh_url": "http://172.16.13.81/api/cli/install.sh",
        "install_ps1_url": "http://172.16.13.81/api/cli/install.ps1",
        "uninstall_sh_url": "http://172.16.13.81/api/cli/uninstall.sh",
        "windows_msi_url": None,
        "available_targets": ["x86_64-unknown-linux-gnu"],
    }
    assert response["Cache-Control"] == "no-store"


@pytest.mark.parametrize(
    "public_url,web_url,app_url,expected",
    [
        (
            "https://cli.example.test:8443/team/",
            "http://web.test",
            "http://app.test",
            "https://cli.example.test:8443/team",
        ),
        ("", "http://web.test/", "http://app.test", "http://web.test"),
        ("", "", "http://app.test/", "http://app.test"),
        ("", "", "", "http://testserver"),
    ],
)
def test_service_url_precedence_and_trailing_slash(api_client, public_url, web_url, app_url, expected):
    with override_settings(KARMA_TEAM_PUBLIC_URL=public_url, WEB_URL=web_url, APP_BASE_URL=app_url):
        response = api_client.get("/api/cli/install-config/")
    assert response.status_code == 200
    assert response.json()["service_url"] == expected
    assert response.json()["install_sh_url"] == f"{expected}/api/cli/install.sh"


@pytest.mark.parametrize(
    "value",
    [
        "ftp://example.test",
        "javascript:alert(1)",
        "//example.test",
        "https://",
        "https://example.test:bad",
        "https://example.test:99999",
        "https://user:secret@example.test",
        "https://example.test?token=secret",
        "https://example.test#fragment",
        "https://example.test/path\nexport EVIL=1",
        "https://example.test/has space",
        "https://example.test/\\path",
    ],
)
def test_invalid_service_url_fails_explicitly_without_exposing_value(api_client, value):
    with override_settings(KARMA_TEAM_PUBLIC_URL=value):
        response = api_client.get("/api/cli/install-config/")
        installer_response = api_client.get("/api/cli/install.sh")
    assert response.status_code == installer_response.status_code == 503
    assert "KARMA_TEAM_PUBLIC_URL" in response.json()["error"]
    assert value not in response.json()["error"]


@pytest.mark.parametrize("target", list(CLI_BINARIES))
def test_binary_download_is_public_and_streams_exact_published_file(api_client, cli_settings, target):
    artifacts, _ = cli_settings
    payload = b"\x00\xffthis-is-a-release-binary"
    _publish(artifacts, target, payload)

    response = api_client.get(f"/api/cli/bin/{target}/")

    assert response.status_code == 200
    assert response.streaming is True
    assert response["Content-Length"] == str(len(payload))
    assert response["Content-Type"] == "application/octet-stream"
    assert response["Content-Disposition"] == f'attachment; filename="{CLI_BINARIES[target]}"'
    assert b"".join(response.streaming_content) == payload
    response.close()


@pytest.mark.parametrize("target", ["x86_64-unknown-linux-gnu", "unknown-target", "..", "%2e%2e%2fsecret"])
def test_missing_and_unrecognized_targets_return_404_without_redirect(api_client, target):
    response = api_client.get(f"/api/cli/bin/{target}/")
    assert response.status_code == 404
    assert "Location" not in response


def test_artifact_symlink_cannot_escape_published_directory(api_client, cli_settings, tmp_path):
    artifacts, _ = cli_settings
    secret = tmp_path / "secret"
    secret.write_bytes(b"not a public release")
    destination = artifacts / "x86_64-unknown-linux-gnu"
    destination.mkdir()
    (destination / "karma-team").symlink_to(secret)

    assert api_client.get("/api/cli/bin/x86_64-unknown-linux-gnu/").status_code == 404
    assert api_client.get("/api/cli/install-config/").json()["available_targets"] == []


def test_msi_only_advertised_when_published(api_client, cli_settings):
    artifacts, _ = cli_settings
    assert api_client.get("/api/cli/windows.msi").status_code == 404
    (artifacts / WINDOWS_MSI).write_bytes(b"msi")

    assert api_client.get("/api/cli/install-config/").json()["windows_msi_url"] == (
        "http://172.16.13.81/api/cli/windows.msi"
    )
    response = api_client.get("/api/cli/windows.msi")
    assert response.status_code == 200
    assert b"".join(response.streaming_content) == b"msi"
    response.close()


@pytest.mark.parametrize("filename", ["install.sh", "install.ps1"])
def test_missing_installer_returns_404(api_client, filename):
    response = api_client.get(f"/api/cli/{filename}")
    assert response.status_code == 404
    assert "not been published" in response.json()["error"]


def test_uninstaller_returns_published_runner_script_without_url_injection(api_client, cli_settings):
    _, installers = cli_settings
    script = b"#!/usr/bin/env sh\nKARMA_TEAM_API_URL='keep-local-config'\nprintf '%s\\n' uninstall\n"
    runner_dir = installers / "runner"
    runner_dir.mkdir()
    (runner_dir / "uninstall.sh").write_bytes(script)

    response = api_client.get("/api/cli/uninstall.sh")

    assert response.status_code == 200
    assert response.content == script
    assert response["Content-Type"] == "text/plain; charset=utf-8"
    assert response["Content-Disposition"] == 'attachment; filename="uninstall.sh"'
    assert response["Cache-Control"] == "no-store"
    assert response["X-Content-Type-Options"] == "nosniff"
    assert b"172.16.13.81" not in response.content


def test_uninstaller_supports_head_and_missing_file_is_404(api_client, cli_settings):
    _, installers = cli_settings
    runner_dir = installers / "runner"
    runner_dir.mkdir()
    (runner_dir / "uninstall.sh").write_bytes(b"#!/usr/bin/env sh\n")

    response = api_client.head("/api/cli/uninstall.sh")
    assert response.status_code == 200
    assert response.content == b""
    assert response["Content-Disposition"] == 'attachment; filename="uninstall.sh"'

    (runner_dir / "uninstall.sh").unlink()
    assert api_client.get("/api/cli/uninstall.sh").status_code == 404


def test_uninstaller_symlink_cannot_escape_published_directory(api_client, cli_settings, tmp_path):
    _, installers = cli_settings
    private_file = tmp_path / "private-uninstaller"
    private_file.write_bytes(b"secret")
    runner_dir = installers / "runner"
    runner_dir.mkdir()
    (runner_dir / "uninstall.sh").symlink_to(private_file)

    assert api_client.get("/api/cli/uninstall.sh").status_code == 404


@pytest.mark.parametrize("override_url", [None, "https://custom.example.test"])
def test_shell_installer_defaults_to_service_and_preserves_client_override(api_client, cli_settings, override_url):
    _, installers = cli_settings
    (installers / "install.sh").write_text('#!/usr/bin/env sh\nset -eu\nprintf "%s" "$KARMA_TEAM_API_URL"\n')
    response = api_client.get("/api/cli/install.sh")
    assert response.status_code == 200
    assert response.content.startswith(b"#!/usr/bin/env sh\n")
    environment = {key: value for key, value in os.environ.items() if key != "KARMA_TEAM_API_URL"}
    if override_url:
        environment["KARMA_TEAM_API_URL"] = override_url
    result = subprocess.run(["sh"], input=response.content, env=environment, check=True, capture_output=True)
    assert result.stdout.decode() == (override_url or "http://172.16.13.81")


def test_shell_installer_safely_quotes_url(api_client, cli_settings, tmp_path):
    _, installers = cli_settings
    (installers / "install.sh").write_text('printf "%s" "$KARMA_TEAM_API_URL"\n')
    marker = tmp_path / "must-not-exist"
    value = f"https://example.test/';$(touch${{IFS}}{marker});'"
    with override_settings(KARMA_TEAM_PUBLIC_URL=value):
        response = api_client.get("/api/cli/install.sh")
    assert response.status_code == 200
    result = subprocess.run(["sh"], input=response.content, env={}, check=True, capture_output=True)
    assert result.stdout.decode() == value
    assert not marker.exists()


def test_powershell_installer_quotes_defaults_without_overriding_environment(api_client, cli_settings):
    _, installers = cli_settings
    (installers / "install.ps1").write_text("Write-Output $env:KARMA_TEAM_API_URL\n")
    with override_settings(KARMA_TEAM_PUBLIC_URL="https://example.test/it's-a-path"):
        response = api_client.get("/api/cli/install.ps1")
    assert response.status_code == 200
    assert response.content.decode() == (
        "if ([string]::IsNullOrWhiteSpace($env:KARMA_TEAM_API_URL)) {\n"
        "    $env:KARMA_TEAM_API_URL = 'https://example.test/it''s-a-path'\n"
        "}\nWrite-Output $env:KARMA_TEAM_API_URL\n"
    )


def test_installer_symlink_cannot_escape_published_directory(api_client, cli_settings, tmp_path):
    _, installers = cli_settings
    private_file = tmp_path / "private-config"
    private_file.write_text("secret")
    (installers / "install.sh").symlink_to(private_file)
    assert api_client.get("/api/cli/install.sh").status_code == 404


@pytest.mark.parametrize(
    "path",
    [
        "install-config/",
        "install.sh",
        "install.ps1",
        "uninstall.sh",
        "bin/x86_64-unknown-linux-gnu/",
        "windows.msi",
    ],
)
def test_download_endpoints_are_read_only(api_client, path):
    assert api_client.post(f"/api/cli/{path}").status_code == 405
