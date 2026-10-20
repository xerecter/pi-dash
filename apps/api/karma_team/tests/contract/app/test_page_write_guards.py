from datetime import date
from unittest.mock import patch

import pytest
from rest_framework import status

from karma_team.app.serializers import PageBinaryUpdateSerializer, PageDetailSerializer
from karma_team.db.models import Page, ProjectMember, ProjectPage, User


def _page(project, user):
    ProjectMember.objects.get_or_create(
        project=project,
        member=user,
        defaults={"role": 20, "is_active": True},
    )
    page = Page.objects.create(
        workspace=project.workspace,
        owned_by=user,
        name="Original",
        description_html="<p>old</p>",
    )
    ProjectPage.objects.create(project=project, page=page, workspace=project.workspace)
    return page


def _page_url(project, page, suffix=""):
    return f"/api/workspaces/{project.workspace.slug}/projects/{project.id}/pages/{page.id}/{suffix}"


@pytest.mark.contract
@pytest.mark.django_db
def test_metadata_write_rechecks_a_concurrent_lock(session_client, project, create_user, monkeypatch):
    page = _page(project, create_user)
    original_is_valid = PageDetailSerializer.is_valid

    def lock_then_validate(serializer, *args, **kwargs):
        Page.objects.filter(pk=page.pk).update(is_locked=True)
        return original_is_valid(serializer, *args, **kwargs)

    monkeypatch.setattr(PageDetailSerializer, "is_valid", lock_then_validate)

    response = session_client.patch(
        _page_url(project, page),
        {"name": "Stale write"},
        format="json",
    )

    assert response.status_code == status.HTTP_400_BAD_REQUEST
    page.refresh_from_db()
    assert page.is_locked is True
    assert page.name == "Original"


@pytest.mark.contract
@pytest.mark.django_db
def test_metadata_write_rechecks_concurrent_private_access(session_client, project, create_user, monkeypatch):
    owner = User.objects.create(
        email="page-owner@example.com",
        username="page-owner",
        first_name="Page",
        last_name="Owner",
    )
    page = _page(project, owner)
    ProjectMember.objects.get_or_create(
        project=project,
        member=create_user,
        defaults={"role": 20, "is_active": True},
    )
    original_is_valid = PageDetailSerializer.is_valid

    def make_private_then_validate(serializer, *args, **kwargs):
        Page.objects.filter(pk=page.pk).update(access=Page.PRIVATE_ACCESS)
        return original_is_valid(serializer, *args, **kwargs)

    monkeypatch.setattr(PageDetailSerializer, "is_valid", make_private_then_validate)

    response = session_client.patch(
        _page_url(project, page),
        {"name": "Unauthorized write"},
        format="json",
    )

    assert response.status_code == status.HTTP_403_FORBIDDEN
    page.refresh_from_db()
    assert page.access == Page.PRIVATE_ACCESS
    assert page.name == "Original"


@pytest.mark.contract
@pytest.mark.django_db
@pytest.mark.parametrize(
    ("state_update", "error_message"),
    [
        ({"is_locked": True}, "PAGE_LOCKED"),
        ({"archived_at": date(2026, 10, 19)}, "PAGE_ARCHIVED"),
    ],
)
def test_body_write_rechecks_concurrent_page_state(
    session_client,
    project,
    create_user,
    monkeypatch,
    state_update,
    error_message,
):
    page = _page(project, create_user)
    original_validate = PageBinaryUpdateSerializer.validate_description_html

    def change_state_then_validate(serializer, value):
        Page.objects.filter(pk=page.pk).update(**state_update)
        return original_validate(serializer, value)

    monkeypatch.setattr(
        PageBinaryUpdateSerializer,
        "validate_description_html",
        change_state_then_validate,
    )

    with (
        patch("karma_team.app.views.page.base.page_transaction.delay") as page_transaction,
        patch("karma_team.app.views.page.base.track_page_version.delay") as track_page_version,
    ):
        response = session_client.patch(
            _page_url(project, page, "description/"),
            {"description_html": "<p>new</p>"},
            format="json",
        )

    assert response.status_code == status.HTTP_400_BAD_REQUEST
    assert response.data["error_message"] == error_message
    page.refresh_from_db()
    assert page.description_html == "<p>old</p>"
    page_transaction.assert_not_called()
    track_page_version.assert_not_called()


@pytest.mark.contract
@pytest.mark.django_db
def test_body_write_updates_only_requested_document_fields(session_client, project, create_user):
    page = _page(project, create_user)
    Page.objects.filter(pk=page.pk).update(color="preserve-me")

    with (
        patch("karma_team.app.views.page.base.page_transaction.delay"),
        patch("karma_team.app.views.page.base.track_page_version.delay"),
    ):
        response = session_client.patch(
            _page_url(project, page, "description/"),
            {"description_html": "<p>new</p>"},
            format="json",
        )

    assert response.status_code == status.HTTP_200_OK
    page.refresh_from_db()
    assert page.description_html == "<p>new</p>"
    assert page.description_stripped == "new"
    assert page.color == "preserve-me"


@pytest.mark.contract
@pytest.mark.django_db
@pytest.mark.parametrize(
    ("method", "suffix", "payload", "initial_locked", "expected_locked", "expected_access"),
    [
        ("post", "lock/", {}, False, True, Page.PUBLIC_ACCESS),
        ("delete", "lock/", {}, True, False, Page.PUBLIC_ACCESS),
        ("post", "access/", {"access": Page.PRIVATE_ACCESS}, False, False, Page.PRIVATE_ACCESS),
    ],
)
def test_page_state_writes_never_replay_stale_content(
    session_client,
    project,
    create_user,
    monkeypatch,
    method,
    suffix,
    payload,
    initial_locked,
    expected_locked,
    expected_access,
):
    page = _page(project, create_user)
    Page.objects.filter(pk=page.pk).update(is_locked=initial_locked)
    original_save = Page.save

    def concurrent_content_then_save(instance, *args, **kwargs):
        Page.objects.filter(pk=instance.pk).update(
            name="Concurrent title",
            description_html="<p>concurrent body</p>",
            description_stripped="concurrent body",
        )
        return original_save(instance, *args, **kwargs)

    monkeypatch.setattr(Page, "save", concurrent_content_then_save)

    response = getattr(session_client, method)(
        _page_url(project, page, suffix),
        payload,
        format="json",
    )

    assert response.status_code == status.HTTP_204_NO_CONTENT
    page.refresh_from_db()
    assert page.name == "Concurrent title"
    assert page.description_html == "<p>concurrent body</p>"
    assert page.description_stripped == "concurrent body"
    assert page.is_locked is expected_locked
    assert page.access == expected_access
