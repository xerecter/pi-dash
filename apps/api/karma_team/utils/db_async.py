# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Run ORM work from async code without parking idle database connections.

``CONN_MAX_AGE`` is 0, so Django only closes a connection when cleanup runs on
the thread that opened it. A bare ``sync_to_async`` ORM call in a streaming
view or long poll can otherwise retain an idle connection for the lifetime of
the request.

This helper mirrors Channels' ``database_sync_to_async`` but does not close a
connection while it is inside an atomic transaction. Closing at that point
would roll back the transaction and leave Django's connection unusable.
"""

from asgiref.sync import SyncToAsync
from django.db import connections


def close_idle_connections() -> None:
    """Close obsolete connections unless they are inside a transaction."""
    for connection in connections.all(initialized_only=True):
        if not connection.in_atomic_block:
            connection.close_if_unusable_or_obsolete()


class DbSyncToAsync(SyncToAsync):
    """``sync_to_async`` that releases database connections around each call."""

    def thread_handler(self, loop, *args, **kwargs):
        close_idle_connections()
        try:
            return super().thread_handler(loop, *args, **kwargs)
        finally:
            close_idle_connections()


# Lowercase alias so call sites read like ``sync_to_async``.
db_sync_to_async = DbSyncToAsync
