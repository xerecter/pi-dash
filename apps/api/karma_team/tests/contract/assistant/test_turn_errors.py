# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""How typed model failures reach the assistant user."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

import pytest

from karma_team.assistant import tasks as assistant_tasks
from karma_team.assistant.errors import ProviderUnreachable, QuotaExceeded
from karma_team.assistant.models import (
    AssistantEvent,
    AssistantMessage,
    AssistantThread,
    AssistantTurn,
    MessageKind,
    MessageStatus,
    TurnStatus,
)
from karma_team.assistant.runtime import events

USAGE_DETAIL = "Plan usage is unavailable. Check Settings > Usage."


def test_typed_errors_keep_their_code_and_detail():
    assert assistant_tasks._classify_error(QuotaExceeded(USAGE_DETAIL)) == (
        "quota_exceeded",
        USAGE_DETAIL,
    )
    assert assistant_tasks._classify_error(ProviderUnreachable("Stream ended early.")) == (
        "provider_unreachable",
        "Stream ended early.",
    )


def test_wrapped_typed_error_is_recognized():
    try:
        try:
            raise QuotaExceeded(USAGE_DETAIL)
        except QuotaExceeded as inner:
            raise RuntimeError("model request failed") from inner
    except RuntimeError as wrapped:
        assert assistant_tasks._classify_error(wrapped) == (
            "quota_exceeded",
            USAGE_DETAIL,
        )


def test_untyped_errors_still_use_text_classification():
    code, _ = assistant_tasks._classify_error(Exception("Connection timed out"))
    assert code == "provider_unreachable"


def test_stream_cancel_check_does_not_churn_database_wrapper(monkeypatch):
    async def one_non_text_event():
        yield object()

    monkeypatch.setattr(assistant_tasks, "_is_cancelled", lambda turn_id: False)

    def unexpected_db_wrapper(*args, **kwargs):
        raise AssertionError("Redis cancellation checks must not close the delta DB connection")

    monkeypatch.setattr(assistant_tasks, "db_sync_to_async", unexpected_db_wrapper)
    ctx = SimpleNamespace(turn=SimpleNamespace(id="turn-id"))

    asyncio.run(assistant_tasks._Streamer(ctx).handle(None, one_non_text_event()))


def test_turn_task_closes_hot_connection_when_cancelled(monkeypatch):
    cleanup_calls = []

    async def cancel_turn(turn_id):
        raise asyncio.CancelledError

    async def close_connection():
        cleanup_calls.append("closed")

    monkeypatch.setattr(assistant_tasks, "_run_turn", cancel_turn)
    monkeypatch.setattr(assistant_tasks, "_close_turn_connection", close_connection)

    with pytest.raises(asyncio.CancelledError):
        assistant_tasks.run_assistant_turn("turn-id")

    assert cleanup_calls == ["closed"]


@pytest.mark.django_db(transaction=True)
def test_typed_error_after_text_delta_reaches_turn_failed_event(world, monkeypatch):
    from pydantic_ai.models.function import FunctionModel

    from karma_team.ee.assistant import model_provider

    async def stream_then_fail(messages, info):
        yield "Hello"
        raise QuotaExceeded(USAGE_DETAIL)

    monkeypatch.setattr(
        model_provider,
        "resolve_model_for_user",
        lambda user: FunctionModel(stream_function=stream_then_fail),
    )
    monkeypatch.setattr(model_provider, "resolve_toolsets_for_user", lambda user: ([], []))
    monkeypatch.setattr(model_provider, "model_label_for_user", lambda user: "test-model")
    monkeypatch.setattr(events, "publish_event", lambda event: None)
    monkeypatch.setattr(assistant_tasks, "_is_cancelled", lambda turn_id: False)

    thread = AssistantThread.objects.create(workspace=world.ws, user=world.member)
    user_message = AssistantMessage.objects.create(thread=thread, seq=1, kind=MessageKind.USER, display_content="Hi")
    turn = AssistantTurn.objects.create(thread=thread, user_message=user_message)
    AssistantThread.objects.filter(pk=thread.pk).update(active_turn=turn)

    assistant_tasks.run_assistant_turn(str(turn.id))

    failed = AssistantEvent.objects.get(thread=thread, kind="turn_failed")
    assert failed.payload == {
        "turn_id": str(turn.id),
        "error_code": "quota_exceeded",
        "detail": USAGE_DETAIL,
    }

    turn.refresh_from_db()
    assert turn.status == TurnStatus.FAILED
    assert turn.error_code == "quota_exceeded"
    assert turn.error_detail == USAGE_DETAIL

    partial = AssistantMessage.objects.get(thread=thread, kind=MessageKind.ASSISTANT)
    assert partial.status == MessageStatus.FAILED
    error_row = AssistantMessage.objects.get(thread=thread, kind=MessageKind.ERROR)
    assert error_row.display_content == USAGE_DETAIL

    thread.refresh_from_db()
    assert thread.active_turn_id is None
