# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import pytest

from karma_team.integrations.git.adapters.base import GitProviderPermissionError
from karma_team.integrations.git.adapters.gitlab import GitLabAdapter
from karma_team.integrations.git.registry import parse_code_review_url, parse_repository_url


pytestmark = pytest.mark.unit


def test_parse_github_repository_and_pull_request():
    repo = parse_repository_url("https://github.com/Acme/Web.git")
    assert repo.provider == "github"
    assert repo.host_url == "https://github.com"
    assert repo.namespace == "acme"
    assert repo.name == "web"
    assert repo.full_name == "acme/web"

    review = parse_code_review_url("https://github.com/Acme/Web/pull/42")
    assert review.provider == "github"
    assert review.namespace == "acme"
    assert review.repo_name == "web"
    assert review.external_iid == "42"
    assert review.url == "https://github.com/acme/web/pull/42"


def test_parse_gitlab_nested_repository_and_merge_request(settings):
    settings.GITLAB_ALLOWED_HOSTS = ["https://gitlab.example.com"]

    repo = parse_repository_url("git@gitlab.example.com:platform/backend/api.git")
    assert repo.provider == "gitlab"
    assert repo.host_url == "https://gitlab.example.com"
    assert repo.namespace == "platform/backend"
    assert repo.name == "api"
    assert repo.full_name == "platform/backend/api"

    review = parse_code_review_url("https://gitlab.example.com/platform/backend/api/-/merge_requests/17")
    assert review.provider == "gitlab"
    assert review.host_url == "https://gitlab.example.com"
    assert review.namespace == "platform/backend"
    assert review.repo_name == "api"
    assert review.external_iid == "17"


def test_parse_http_gitlab_repository_preserves_configured_scheme(settings):
    settings.GITLAB_ALLOWED_HOSTS = ["http://gitlab.example.com"]

    repo = parse_repository_url("http://gitlab.example.com/platform/backend/api.git")

    assert repo.provider == "gitlab"
    assert repo.host_url == "http://gitlab.example.com"
    assert repo.namespace == "platform/backend"
    assert repo.name == "api"
    assert repo.full_name == "platform/backend/api"


def test_parse_gitlab_ssh_repository_uses_http_allowlisted_host(settings):
    settings.GITLAB_ALLOWED_HOSTS = ["http://gitlab.example.com"]

    repo = parse_repository_url("git@gitlab.example.com:team/web.git")

    assert repo.provider == "gitlab"
    assert repo.host_url == "http://gitlab.example.com"
    assert repo.clone_url == "git@gitlab.example.com:team/web.git"


def test_parse_gitlab_ssh_url_uses_https_when_both_schemes_are_allowlisted(settings):
    settings.GITLAB_ALLOWED_HOSTS = [
        "http://gitlab.example.com",
        "https://gitlab.example.com",
    ]

    repo = parse_repository_url("ssh://git@gitlab.example.com/team/web.git")

    assert repo.provider == "gitlab"
    assert repo.host_url == "https://gitlab.example.com"
    assert repo.clone_url == "ssh://git@gitlab.example.com/team/web.git"


@pytest.mark.parametrize("scheme", ["http", "https"])
def test_parse_self_managed_gitlab_merge_request_for_configured_deployment_host(settings, scheme):
    settings.GITLAB_ALLOWED_HOSTS = [
        "http://gitlab.laiyeai.com",
        "https://gitlab.laiyeai.com",
    ]

    review = parse_code_review_url(
        f"{scheme}://gitlab.laiyeai.com/xuhaoxuan/kt-test2/-/merge_requests/2"
    )

    assert review is not None
    assert review.provider == "gitlab"
    assert review.host_url == f"{scheme}://gitlab.laiyeai.com"
    assert review.namespace == "xuhaoxuan"
    assert review.repo_name == "kt-test2"
    assert review.external_iid == "2"


def test_parse_http_merge_request_keeps_legacy_https_allowlist_compatibility(settings):
    settings.GITLAB_ALLOWED_HOSTS = ["https://gitlab.example.com"]

    review = parse_code_review_url("http://gitlab.example.com/acme/web/-/merge_requests/17")

    assert review is not None
    assert review.host_url == "https://gitlab.example.com"
    assert review.url == "https://gitlab.example.com/acme/web/-/merge_requests/17"


def test_gitlab_rejects_unconfigured_hosts_but_generic_repository_is_allowed(settings):
    settings.GITLAB_ALLOWED_HOSTS = []
    settings.GITLAB_HOST = "https://gitlab.com"

    repository = parse_repository_url("https://evil.example.com/acme/web")
    assert repository.provider == "generic"
    assert repository.host_url == "https://evil.example.com"
    assert parse_code_review_url("https://evil.example.com/acme/web/-/merge_requests/1") is None


@pytest.mark.parametrize(
    "url, host_url, namespace, name",
    [
        ("https://gitea.example.com/team/web.git", "https://gitea.example.com", "team", "web"),
        ("http://10.0.0.8:3000/team/web", "http://10.0.0.8:3000", "team", "web"),
        ("ssh://git@git.example.com/team/web.git", "ssh://git.example.com", "team", "web"),
        ("git://git.example.com/team/web.git", "git://git.example.com", "team", "web"),
        ("git@git.example.com:team/web.git", "ssh://git.example.com", "team", "web"),
    ],
)
def test_parse_generic_git_repository_without_provider_allowlist(url, host_url, namespace, name):
    repo = parse_repository_url(url)
    assert repo.provider == "generic"
    assert repo.host_url == host_url
    assert repo.namespace == namespace
    assert repo.name == name
    assert repo.full_name == f"{namespace}/{name}"


@pytest.mark.parametrize("url", ["", "--upload-pack=evil", "https://", "https://host/", "https://host/a\nrm -rf"])
def test_parse_generic_git_repository_rejects_invalid_remote(url):
    assert parse_repository_url(url) is None


def test_gitlab_account_verify_allows_gitlab_com(settings, mocker):
    settings.GITLAB_ALLOWED_HOSTS = []
    request = mocker.patch("karma_team.integrations.git.adapters.gitlab.requests.request")
    response = mocker.Mock(status_code=200, headers={}, text="")
    response.json.return_value = {"id": 1, "username": "octo"}
    response.raise_for_status.return_value = None
    request.return_value = response

    identity = GitLabAdapter().verify_provider_account({"token": "token", "host_url": "https://gitlab.com"})

    assert identity["username"] == "octo"
    request.assert_called_once()
    assert request.call_args.args[:2] == ("GET", "https://gitlab.com/api/v4/user")
    assert request.call_args.kwargs["allow_redirects"] is False


def test_gitlab_account_verify_allows_configured_self_managed_host(settings, mocker):
    settings.GITLAB_ALLOWED_HOSTS = ["https://gitlab.example.com"]
    request = mocker.patch("karma_team.integrations.git.adapters.gitlab.requests.request")
    response = mocker.Mock(status_code=200, headers={}, text="")
    response.json.return_value = {"id": 2, "username": "self-managed"}
    response.raise_for_status.return_value = None
    request.return_value = response

    identity = GitLabAdapter().verify_provider_account({"token": "token", "host_url": "gitlab.example.com"})

    assert identity["username"] == "self-managed"
    request.assert_called_once()
    assert request.call_args.args[:2] == ("GET", "https://gitlab.example.com/api/v4/user")
    assert request.call_args.kwargs["allow_redirects"] is False


@pytest.mark.parametrize(
    "host_url",
    [
        "https://evil.example.com",
        "http://localhost:8080",
        "http://169.254.169.254",
    ],
)
def test_gitlab_account_verify_rejects_unconfigured_hosts_before_http(settings, mocker, host_url):
    settings.GITLAB_ALLOWED_HOSTS = []
    request = mocker.patch("karma_team.integrations.git.adapters.gitlab.requests.request")

    with pytest.raises(GitProviderPermissionError, match="GitLab host is not allowed"):
        GitLabAdapter().verify_provider_account({"token": "token", "host_url": host_url})

    request.assert_not_called()


def test_gitlab_account_verify_allows_allowlisted_http_host(settings, mocker):
    settings.GITLAB_ALLOWED_HOSTS = ["http://gitlab.example.com"]
    request = mocker.patch("karma_team.integrations.git.adapters.gitlab.requests.request")
    response = mocker.Mock(status_code=200, headers={}, text="")
    response.json.return_value = {"id": 3, "username": "http-self-managed"}
    response.raise_for_status.return_value = None
    request.return_value = response

    identity = GitLabAdapter().verify_provider_account(
        {"token": "token", "host_url": "http://gitlab.example.com"}
    )

    assert identity["username"] == "http-self-managed"
    request.assert_called_once()
    assert request.call_args.args[:2] == ("GET", "http://gitlab.example.com/api/v4/user")
    assert request.call_args.kwargs["allow_redirects"] is False


def test_gitlab_account_verify_rejects_redirects(settings, mocker):
    settings.GITLAB_ALLOWED_HOSTS = []
    request = mocker.patch("karma_team.integrations.git.adapters.gitlab.requests.request")
    response = mocker.Mock(status_code=302, headers={"Location": "https://evil.example.com"}, text="")
    response.raise_for_status.return_value = None
    request.return_value = response

    with pytest.raises(GitProviderPermissionError, match="redirects are not followed"):
        GitLabAdapter().verify_provider_account({"token": "token", "host_url": "https://gitlab.com"})

    request.assert_called_once()
    assert request.call_args.kwargs["allow_redirects"] is False
