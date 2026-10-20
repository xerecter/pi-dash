# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

import asyncio
from types import SimpleNamespace

import pytest

from karma_team.assistant.models import (
    AssistantEvent,
    AssistantThread,
    AssistantTurn,
    MessageKind,
)
from karma_team.assistant.runtime import events
from karma_team.assistant.views import events as event_views

pytestmark = pytest.mark.django_db


@pytest.fixture
def thread(world):
    return AssistantThread.objects.create(workspace=world.ws, user=world.member)


def test_message_seq_increments_per_thread(thread):
    m1 = events.create_message(thread, MessageKind.USER, display_content="a")
    m2 = events.create_message(thread, MessageKind.ASSISTANT, display_content="b")
    assert m1.seq == 1
    assert m2.seq == 2


def test_event_seq_independent_from_message_seq(thread):
    events.create_message(thread, MessageKind.USER, display_content="a")  # message seq 1
    e1 = events.append_event(thread, "turn_started", payload={"x": 1})
    e2 = events.append_event(thread, "assistant_delta", payload={"params": {"delta": "hi"}})
    # event seq starts at 1 independently of message seq
    assert e1.seq == 1
    assert e2.seq == 2


def test_message_envelope_shape(thread):
    msg = events.create_message(thread, MessageKind.ASSISTANT, display_content="hello")
    env = events.message_envelope(msg)
    assert env["role"] == "assistant"  # kind -> role
    assert env["content"] == "hello"  # display_content -> content
    assert set(env.keys()) >= {"id", "role", "content", "status", "seq", "turn_id", "payload", "created_at"}


def test_prune_turn_deltas_only_removes_deltas(thread):
    turn = AssistantTurn.objects.create(thread=thread)
    events.append_event(thread, "assistant_delta", payload={"params": {"delta": "x"}}, turn=turn)
    events.append_event(thread, "assistant_delta", payload={"params": {"delta": "y"}}, turn=turn)
    events.append_event(thread, "turn_completed", payload={}, turn=turn)

    events.prune_turn_deltas(turn)

    kinds = set(AssistantEvent.objects.filter(turn=turn).values_list("kind", flat=True))
    assert "assistant_delta" not in kinds
    assert "turn_completed" in kinds


def _direct_sync_to_async(func):
    async def call(*args, **kwargs):
        return func(*args, **kwargs)

    return call


def _event_stream_request():
    return SimpleNamespace(GET={}, user=SimpleNamespace(is_authenticated=True))


def _prepare_event_stream(monkeypatch, pubsub, cleanup_calls):
    class FakeRedis:
        def pubsub(self, ignore_subscribe_messages=True):
            assert ignore_subscribe_messages is True
            return pubsub

    monkeypatch.setattr(event_views, "db_sync_to_async", _direct_sync_to_async)
    monkeypatch.setattr(
        event_views,
        "_resolve",
        lambda request, slug, thread_id: (request.user, SimpleNamespace(id=thread_id)),
    )
    monkeypatch.setattr(event_views, "_replay", lambda thread_id, after: [])
    monkeypatch.setattr(event_views, "async_redis_instance", lambda: FakeRedis())
    monkeypatch.setattr(
        event_views,
        "close_idle_connections",
        lambda: cleanup_calls.append("closed"),
    )


def test_assistant_sse_cancellation_cleans_up_subscription(monkeypatch):
    entered_read = asyncio.Event()
    cleanup_calls = []

    class FakePubsub:
        def __init__(self):
            self.subscriptions = []
            self.unsubscriptions = []
            self.closed = False

        async def subscribe(self, channel):
            self.subscriptions.append(channel)

        async def get_message(self, timeout):
            entered_read.set()
            await asyncio.Future()

        async def unsubscribe(self, channel):
            self.unsubscriptions.append(channel)

        async def aclose(self):
            self.closed = True

    pubsub = FakePubsub()
    _prepare_event_stream(monkeypatch, pubsub, cleanup_calls)

    async def scenario():
        response = await event_views.assistant_event_stream(_event_stream_request(), "workspace", "thread-id")
        stream = response.streaming_content
        next_event = asyncio.create_task(stream.__anext__())
        await asyncio.wait_for(entered_read.wait(), timeout=1)
        next_event.cancel()
        with pytest.raises(asyncio.CancelledError):
            await next_event

    asyncio.run(scenario())

    assert len(pubsub.subscriptions) == 1
    assert pubsub.unsubscriptions == pubsub.subscriptions
    assert pubsub.closed is True
    assert cleanup_calls == ["closed"]


def test_assistant_sse_subscription_failure_still_closes_pubsub(monkeypatch):
    cleanup_calls = []

    class FailingPubsub:
        closed = False
        unsubscribed = False

        async def subscribe(self, channel):
            raise RuntimeError("subscription failed")

        async def unsubscribe(self, channel):
            self.unsubscribed = True

        async def aclose(self):
            self.closed = True

    pubsub = FailingPubsub()
    _prepare_event_stream(monkeypatch, pubsub, cleanup_calls)
    monkeypatch.setattr(event_views.logger, "exception", lambda *args, **kwargs: None)

    async def scenario():
        response = await event_views.assistant_event_stream(_event_stream_request(), "workspace", "thread-id")
        with pytest.raises(StopAsyncIteration):
            await response.streaming_content.__anext__()

    asyncio.run(scenario())

    assert pubsub.unsubscribed is False
    assert pubsub.closed is True
    assert cleanup_calls == ["closed"]
