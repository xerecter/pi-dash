# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from __future__ import annotations

import re
from urllib.parse import urlparse

from karma_team.integrations.git.adapters.github import GitHubAdapter
from karma_team.integrations.git.adapters.gitlab import GitLabAdapter
from karma_team.integrations.git.dtos import ParsedCodeReview, ParsedRepository


_ADAPTERS = {
    GitHubAdapter.key: GitHubAdapter(),
    GitLabAdapter.key: GitLabAdapter(),
}

_GENERIC_GIT_SCHEMES = {"git", "http", "https", "ssh"}
_SCP_STYLE_RE = re.compile(r"^(?P<user>[A-Za-z0-9._-]+)@(?P<host>[^:/\s]+):(?P<path>.+)$")


def _strip_git_suffix(path: str) -> str:
    path = path.strip("/")
    return path[:-4] if path.lower().endswith(".git") else path


def _generic_repository_url(url: str) -> ParsedRepository | None:
    """Parse a Git remote without assuming a hosting provider.

    Provider adapters are still tried first so GitHub and GitLab keep their
    account verification and issue-sync behavior. This fallback only records
    enough metadata for the runner to clone arbitrary Git hosts; it deliberately
    does not attempt an unauthenticated network request from the API.
    """

    candidate = (url or "").strip()
    if not candidate or candidate.startswith("-") or any(char.isspace() or ord(char) < 32 for char in candidate):
        return None

    scp_match = _SCP_STYLE_RE.match(candidate)
    if scp_match:
        host = scp_match.group("host").lower()
        path = _strip_git_suffix(scp_match.group("path"))
        if not host or not path:
            return None
        host_url = f"ssh://{host}"
        clone_url = candidate
    else:
        parsed = urlparse(candidate)
        if parsed.scheme.lower() not in _GENERIC_GIT_SCHEMES or not parsed.hostname or not parsed.path.strip("/"):
            return None
        try:
            port = parsed.port
        except ValueError:
            return None
        host = parsed.hostname.lower()
        host_with_port = f"{host}:{port}" if port else host
        host_url = f"{parsed.scheme.lower()}://{host_with_port}"
        path = _strip_git_suffix(parsed.path)
        if not path:
            return None
        clone_url = candidate

    name = path.rsplit("/", 1)[-1]
    namespace = path.rsplit("/", 1)[0] if "/" in path else ""
    if not name:
        return None
    return ParsedRepository(
        provider="generic",
        host_url=host_url,
        namespace=namespace,
        name=name,
        full_name=path,
        clone_url=clone_url,
    )


def get_adapter(provider: str):
    adapter = _ADAPTERS.get((provider or "").lower())
    if adapter is None:
        raise KeyError(f"Unsupported Git provider: {provider}")
    return adapter


def all_adapters():
    return list(_ADAPTERS.values())


def parse_repository_url(url: str) -> ParsedRepository | None:
    for adapter in all_adapters():
        parsed = adapter.parse_repo_url(url)
        if parsed is not None:
            return parsed
    return _generic_repository_url(url)


def parse_code_review_url(url: str) -> ParsedCodeReview | None:
    for adapter in all_adapters():
        parsed = adapter.parse_code_review_url(url)
        if parsed is not None:
            return parsed
    return None


def provider_payload() -> list[dict]:
    return [
        {
            "key": adapter.key,
            "display_name": adapter.display_name,
            "code_review_term": adapter.code_review_term,
        }
        for adapter in all_adapters()
    ]
