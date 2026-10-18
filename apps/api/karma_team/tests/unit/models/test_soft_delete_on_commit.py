"""Transaction-boundary contracts for generic soft-delete cascades."""

from unittest.mock import patch

import pytest
from django.db import transaction


@pytest.mark.unit
def test_soft_delete_dispatches_related_cleanup_only_after_commit(
    db, project, django_capture_on_commit_callbacks
):
    with patch("karma_team.db.mixins.soft_delete_related_objects.delay") as delay:
        with django_capture_on_commit_callbacks(execute=True) as callbacks:
            with transaction.atomic():
                project.delete()
                delay.assert_not_called()

        assert len(callbacks) == 1
        delay.assert_called_once_with(
            project._meta.app_label,
            project._meta.model_name,
            project.pk,
            using=None,
        )


@pytest.mark.unit
def test_soft_delete_does_not_dispatch_related_cleanup_after_rollback(
    db, project, django_capture_on_commit_callbacks
):
    class RollBackDelete(Exception):
        pass

    with patch("karma_team.db.mixins.soft_delete_related_objects.delay") as delay:
        with django_capture_on_commit_callbacks(execute=True) as callbacks:
            with pytest.raises(RollBackDelete):
                with transaction.atomic():
                    project.delete()
                    raise RollBackDelete

        assert callbacks == []
        delay.assert_not_called()
