# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from contextlib import contextmanager

from django.db import router, transaction


@contextmanager
def locked_issue_for_update(issue, *, using=None):
    """Refresh a work item under a row lock before applying an explicit edit.

    ModelSerializer.update() and soft deletion save the whole model. An instance
    loaded before a concurrent edit must not write its old branch (or any other
    omitted field) back. Keep the original instance so annotations and dispatch
    flags attached by the view survive; refresh_from_db clears stale FK caches.
    Callers must apply their changes *inside* this context, after the refresh.
    """
    using = using or router.db_for_write(type(issue), instance=issue)
    with transaction.atomic(using=using):
        type(issue).all_objects.using(using).select_for_update().only("pk").get(pk=issue.pk)
        issue.refresh_from_db(using=using)
        yield issue
