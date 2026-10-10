# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Public, self-hosted CLI installers and explicitly published binaries."""

import shlex
from pathlib import Path
from urllib.parse import urlsplit

from django.conf import settings
from django.core.exceptions import ImproperlyConfigured
from django.http import FileResponse, HttpResponse, JsonResponse
from django.views.decorators.http import require_safe


CLI_BINARIES = {
    "x86_64-unknown-linux-gnu": "karma-team",
    "aarch64-unknown-linux-gnu": "karma-team",
    "x86_64-unknown-linux-musl": "karma-team",
    "aarch64-unknown-linux-musl": "karma-team",
    "x86_64-apple-darwin": "karma-team",
    "aarch64-apple-darwin": "karma-team",
    "x86_64-pc-windows-msvc": "karma-team.exe",
}
WINDOWS_MSI = "karma-team-x86_64-pc-windows-msvc.msi"


def _service_url(request):
    for key in ("KARMA_TEAM_PUBLIC_URL", "WEB_URL", "APP_BASE_URL"):
        value = getattr(settings, key, "")
        if value:
            break
    else:
        key = "request origin"
        value = request.build_absolute_uri("/")

    error = f"{key} must be an absolute HTTP(S) URL without credentials, query, or fragment."
    try:
        parsed = urlsplit(value)
        if (
            parsed.scheme not in ("http", "https")
            or not parsed.hostname
            or parsed.username is not None
            or parsed.password is not None
            or parsed.query
            or parsed.fragment
            or "?" in value
            or "#" in value
            or "\\" in value
            or any(character.isspace() or ord(character) < 32 or ord(character) == 127 for character in value)
        ):
            raise ValueError(error)
        # Accessing port also validates nonnumeric and out-of-range values.
        parsed.port
    except (TypeError, ValueError) as exc:
        raise ImproperlyConfigured(error) from exc
    return value.rstrip("/")


def _published_file(directory, relative_path):
    try:
        root = Path(directory).resolve()
        candidate = (root / relative_path).resolve()
        if candidate.is_relative_to(root) and candidate.is_file():
            return candidate
    except (OSError, RuntimeError):
        pass
    return None


def _uncached(response):
    response["Cache-Control"] = "no-store"
    response["X-Content-Type-Options"] = "nosniff"
    return response


def _download(relative_path, filename):
    artifact = _published_file(settings.KARMA_TEAM_CLI_ARTIFACT_DIR, relative_path)
    if artifact is not None:
        try:
            return _uncached(
                FileResponse(
                    artifact.open("rb"),
                    as_attachment=True,
                    filename=filename,
                    content_type="application/octet-stream",
                )
            )
        except OSError:
            pass
    return _uncached(
        JsonResponse({"error": "The requested CLI artifact has not been published on this server."}, status=404)
    )


@require_safe
def cli_install_config(request):
    try:
        service_url = _service_url(request)
    except ImproperlyConfigured as exc:
        return _uncached(JsonResponse({"error": str(exc)}, status=503))
    artifact_dir = settings.KARMA_TEAM_CLI_ARTIFACT_DIR
    return _uncached(
        JsonResponse(
            {
                "service_url": service_url,
                "install_sh_url": f"{service_url}/api/cli/install.sh",
                "install_ps1_url": f"{service_url}/api/cli/install.ps1",
                "uninstall_sh_url": f"{service_url}/api/cli/uninstall.sh",
                "windows_msi_url": (
                    f"{service_url}/api/cli/windows.msi" if _published_file(artifact_dir, WINDOWS_MSI) else None
                ),
                "available_targets": [
                    target
                    for target, filename in CLI_BINARIES.items()
                    if _published_file(artifact_dir, Path(target) / filename)
                ],
            }
        )
    )


@require_safe
def cli_binary(request, target):
    filename = CLI_BINARIES.get(target)
    if filename is None:
        return _uncached(JsonResponse({"error": "Unsupported CLI target."}, status=404))
    return _download(Path(target) / filename, filename)


@require_safe
def cli_windows_msi(request):
    return _download(WINDOWS_MSI, WINDOWS_MSI)


def _published_installer(filename):
    """Return an installer below the configured publication directory."""
    installer = _published_file(settings.KARMA_TEAM_CLI_INSTALLER_DIR, Path("runner") / filename)
    if installer is None:
        installer = _published_file(settings.KARMA_TEAM_CLI_INSTALLER_DIR, filename)
    return installer


@require_safe
def cli_installer(request, filename):
    if filename not in ("install.sh", "install.ps1"):
        return _uncached(JsonResponse({"error": "Unsupported CLI installer."}, status=404))
    try:
        service_url = _service_url(request)
    except ImproperlyConfigured as exc:
        return _uncached(JsonResponse({"error": str(exc)}, status=503))
    installer = _published_installer(filename)
    try:
        script = installer.read_text(encoding="utf-8-sig") if installer else None
    except (OSError, UnicodeError):
        script = None
    if script is None:
        return _uncached(
            JsonResponse({"error": "The CLI installer has not been published on this server."}, status=404)
        )

    if filename == "install.sh":
        bootstrap = (
            f'if [ -z "${{KARMA_TEAM_API_URL:-}}" ]; then\n  export KARMA_TEAM_API_URL={shlex.quote(service_url)}\nfi\n'
        )
        if script.startswith("#!"):
            shebang, _, body = script.partition("\n")
            script = f"{shebang}\n{bootstrap}{body}"
        else:
            script = bootstrap + script
    else:
        quoted_url = service_url.replace("'", "''")
        script = (
            "if ([string]::IsNullOrWhiteSpace($env:KARMA_TEAM_API_URL)) {\n"
            f"    $env:KARMA_TEAM_API_URL = '{quoted_url}'\n"
            "}\n" + script
        )
    response = HttpResponse(script, content_type="text/plain; charset=utf-8")
    response["Content-Disposition"] = f'attachment; filename="{filename}"'
    return _uncached(response)


@require_safe
def cli_uninstaller(request):
    """Serve the self-hosted uninstaller without changing its local config."""
    installer = _published_installer("uninstall.sh")
    try:
        script = installer.read_bytes() if installer else None
    except OSError:
        script = None
    if script is None:
        return _uncached(
            JsonResponse({"error": "The CLI uninstaller has not been published on this server."}, status=404)
        )
    response = HttpResponse(script, content_type="text/plain; charset=utf-8")
    response["Content-Disposition"] = 'attachment; filename="uninstall.sh"'
    return _uncached(response)
