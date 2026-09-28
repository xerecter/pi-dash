# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from __future__ import annotations

import pytest

from karma_team.db.models import GitProviderAccount, GitRepositoryBinding
from karma_team.integrations.git.adapters.base import GitProviderNotFoundError
from karma_team.integrations.git.dtos import RemoteRepository
from karma_team.integrations.git.services import (
    GitIntegrationError,
    ProviderAccountAmbiguous,
    bind_repository,
    set_binding_sync_enabled,
)


pytestmark = [pytest.mark.unit, pytest.mark.django_db]


class _RepositoryProbeAdapter:
    def __init__(self, visible_auth_types: set[str]):
        self.visible_auth_types = visible_auth_types
        self.calls: list[str] = []

    def get_repository(self, credential: dict, parsed):
        auth_type = credential.get("auth_type") or ""
        self.calls.append(auth_type)
        if auth_type not in self.visible_auth_types:
            raise GitProviderNotFoundError("repository not visible")
        return RemoteRepository(
            provider=parsed.provider,
            external_id="123",
            namespace=parsed.namespace,
            name=parsed.name,
            full_name=parsed.full_name,
            web_url=f"{parsed.host_url}/{parsed.full_name}",
            clone_url_http=f"{parsed.host_url}/{parsed.full_name}.git",
            default_branch="main",
            is_private=True,
        )


def _account(workspace, *, auth_type: str, external_id: str, write_comments: bool):
    return GitProviderAccount.objects.create(
        workspace=workspace,
        provider=GitProviderAccount.Provider.GITHUB,
        host_url="https://github.com",
        auth_type=auth_type,
        external_account_id=external_id,
        display_name=external_id,
        capabilities={
            "read_repositories": True,
            "read_issues": True,
            "write_comments": write_comments,
            "manage_webhooks": auth_type == GitProviderAccount.AuthType.GITHUB_APP,
            "clone": False,
        },
        credential_config={
            "auth_type": auth_type,
            "host_url": "https://github.com",
            "token": auth_type,
        },
        status=GitProviderAccount.Status.CONNECTED,
    )


def test_bind_repository_prefers_github_pat_when_pat_and_app_can_access(
    monkeypatch,
    workspace,
    project,
    create_user,
):
    pat = _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.PAT,
        external_id="pat",
        write_comments=True,
    )
    _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.GITHUB_APP,
        external_id="installation",
        write_comments=False,
    )
    adapter = _RepositoryProbeAdapter(
        {GitProviderAccount.AuthType.PAT, GitProviderAccount.AuthType.GITHUB_APP}
    )
    monkeypatch.setattr("karma_team.integrations.git.services.get_adapter", lambda _provider: adapter)

    binding, clone_url = bind_repository(
        workspace_slug=workspace.slug,
        project_id=project.id,
        actor=create_user,
        raw_url="https://github.com/acme/web",
    )

    assert binding.provider_account_id == pat.id
    assert clone_url == "https://github.com/acme/web.git"
    assert GitRepositoryBinding.objects.get(project=project).provider_account_id == pat.id


def test_bind_repository_uses_github_app_when_only_app_can_access(
    monkeypatch,
    workspace,
    project,
    create_user,
):
    _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.PAT,
        external_id="pat",
        write_comments=True,
    )
    app = _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.GITHUB_APP,
        external_id="installation",
        write_comments=False,
    )
    adapter = _RepositoryProbeAdapter({GitProviderAccount.AuthType.GITHUB_APP})
    monkeypatch.setattr("karma_team.integrations.git.services.get_adapter", lambda _provider: adapter)

    binding, _clone_url = bind_repository(
        workspace_slug=workspace.slug,
        project_id=project.id,
        actor=create_user,
        raw_url="https://github.com/acme/web",
    )

    assert binding.provider_account_id == app.id
    assert adapter.calls == [GitProviderAccount.AuthType.PAT, GitProviderAccount.AuthType.GITHUB_APP]


def test_bind_repository_stays_ambiguous_when_multiple_pat_accounts_can_access(
    monkeypatch,
    workspace,
    project,
    create_user,
):
    _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.PAT,
        external_id="pat-1",
        write_comments=True,
    )
    _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.PAT,
        external_id="pat-2",
        write_comments=True,
    )
    adapter = _RepositoryProbeAdapter({GitProviderAccount.AuthType.PAT})
    monkeypatch.setattr("karma_team.integrations.git.services.get_adapter", lambda _provider: adapter)

    with pytest.raises(ProviderAccountAmbiguous):
        bind_repository(
            workspace_slug=workspace.slug,
            project_id=project.id,
            actor=create_user,
            raw_url="https://github.com/acme/web",
        )


def test_bind_repository_honors_explicit_provider_account_id(
    monkeypatch,
    workspace,
    project,
    create_user,
):
    _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.PAT,
        external_id="pat",
        write_comments=True,
    )
    app = _account(
        workspace,
        auth_type=GitProviderAccount.AuthType.GITHUB_APP,
        external_id="installation",
        write_comments=False,
    )
    adapter = _RepositoryProbeAdapter(
        {GitProviderAccount.AuthType.PAT, GitProviderAccount.AuthType.GITHUB_APP}
    )
    monkeypatch.setattr("karma_team.integrations.git.services.get_adapter", lambda _provider: adapter)

    binding, _clone_url = bind_repository(
        workspace_slug=workspace.slug,
        project_id=project.id,
        actor=create_user,
        raw_url="https://github.com/acme/web",
        provider_account_id=app.id,
    )

    assert binding.provider_account_id == app.id
    assert adapter.calls == [GitProviderAccount.AuthType.GITHUB_APP]


def test_bind_repository_accepts_generic_git_remote_without_provider_account(project, create_user):
    binding, clone_url = bind_repository(
        workspace_slug=project.workspace.slug,
        project_id=project.id,
        actor=create_user,
        raw_url="https://gitea.example.com/team/web.git",
    )

    assert binding.repository.provider == "generic"
    assert binding.provider_account is None
    assert clone_url == "https://gitea.example.com/team/web.git"
    project.refresh_from_db()
    assert project.repo_url == clone_url
    assert binding.is_sync_enabled is False


def test_generic_git_binding_cannot_enable_provider_issue_sync(project, create_user):
    binding, _ = bind_repository(
        workspace_slug=project.workspace.slug,
        project_id=project.id,
        actor=create_user,
        raw_url="git@git.example.com:team/web.git",
    )

    with pytest.raises(GitIntegrationError, match="unavailable for generic Git remotes"):
        set_binding_sync_enabled(
            workspace_slug=project.workspace.slug,
            project_id=project.id,
            enabled=True,
        )

    binding.refresh_from_db()
    assert binding.is_sync_enabled is False
