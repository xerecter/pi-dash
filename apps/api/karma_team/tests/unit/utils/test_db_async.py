# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

from __future__ import annotations

import asyncio
import threading

import pytest
from django.db import connection, transaction

from karma_team.utils import db_async


class _Connection:
    def __init__(self, *, in_atomic_block: bool):
        self.in_atomic_block = in_atomic_block
        self.close_calls = 0

    def close_if_unusable_or_obsolete(self):
        self.close_calls += 1


@pytest.mark.unit
def test_close_idle_connections_skips_atomic_connections(monkeypatch):
    idle = _Connection(in_atomic_block=False)
    atomic = _Connection(in_atomic_block=True)
    calls = []

    def all_connections(*, initialized_only):
        calls.append(initialized_only)
        return [idle, atomic]

    monkeypatch.setattr(db_async.connections, "all", all_connections)

    db_async.close_idle_connections()

    assert calls == [True]
    assert idle.close_calls == 1
    assert atomic.close_calls == 0


@pytest.mark.unit
def test_db_sync_to_async_closes_before_and_after_call(monkeypatch):
    close_calls = []
    monkeypatch.setattr(db_async, "close_idle_connections", lambda: close_calls.append("close"))

    result = asyncio.run(db_async.db_sync_to_async(lambda: "result")())

    assert result == "result"
    assert close_calls == ["close", "close"]


@pytest.mark.unit
def test_db_sync_to_async_closes_after_exception(monkeypatch):
    close_calls = []
    monkeypatch.setattr(db_async, "close_idle_connections", lambda: close_calls.append("close"))

    def fail():
        raise RuntimeError("failed")

    with pytest.raises(RuntimeError, match="failed"):
        asyncio.run(db_async.db_sync_to_async(fail)())

    assert close_calls == ["close", "close"]


@pytest.mark.unit
def test_db_sync_to_async_closes_after_task_cancellation(monkeypatch):
    close_calls = []
    started = threading.Event()
    release = threading.Event()
    cleaned_up = threading.Event()

    def close_connections():
        close_calls.append("close")
        if len(close_calls) == 2:
            cleaned_up.set()

    def block_until_cancelled():
        started.set()
        release.wait(timeout=2)

    monkeypatch.setattr(db_async, "close_idle_connections", close_connections)

    async def scenario():
        task = asyncio.create_task(db_async.db_sync_to_async(block_until_cancelled)())
        while not started.is_set():
            await asyncio.sleep(0)
        task.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert await asyncio.to_thread(cleaned_up.wait, 2)

    asyncio.run(scenario())

    assert close_calls == ["close", "close"]


@pytest.mark.unit
@pytest.mark.django_db(transaction=True)
def test_close_idle_connections_preserves_real_atomic_transaction():
    with transaction.atomic():
        with connection.cursor() as cursor:
            cursor.execute("SELECT 1")
            assert cursor.fetchone() == (1,)

        raw_connection = connection.connection
        assert connection.in_atomic_block is True

        db_async.close_idle_connections()

        assert connection.connection is raw_connection
        assert connection.in_atomic_block is True
        with connection.cursor() as cursor:
            cursor.execute("SELECT 1")
            assert cursor.fetchone() == (1,)
