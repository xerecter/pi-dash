# Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
# SPDX-License-Identifier: AGPL-3.0-only
# See the LICENSE file for details.

"""Unit tests for the per-dev-machine Redis Streams outbox.

Mirror of the per-runner outbox tests: exercise enqueue routing
(live vs offline vs reject), key namespacing, and ack — all against a
minimal fake Redis so no live server is needed.
"""

from __future__ import annotations

import pytest

from karma_team.runner.services import machine_outbox
from karma_team.runner.services.machine_outbox import MachineOfflineError


class _FakeRedis:
    def __init__(self):
        self.streams: dict[str, list] = {}
        self.acked: list = []
        self._seq = 0

    def xadd(self, key, fields, maxlen=None, approximate=None):
        self._seq += 1
        sid = f"{self._seq}-0".encode()
        self.streams.setdefault(key, []).append((sid, dict(fields)))
        return sid

    def xgroup_create(self, name=None, groupname=None, id=None, mkstream=None):
        return True

    def expire(self, key, ttl):
        return True

    def xack(self, key, group, *ids):
        self.acked.extend(ids)
        return len(ids)


class _ClaimRedis(_FakeRedis):
    def __init__(self, *, results=None, error=None):
        super().__init__()
        self.results = list(results or [])
        self.error = error
        self.deleted_consumers: list[tuple[str, str, str]] = []
        self.claim_calls: list[dict] = []

    def xautoclaim(self, **kwargs):
        self.claim_calls.append(kwargs)
        if self.error is not None:
            raise self.error
        if self.results:
            return self.results.pop(0)
        return (b"0-0", [])

    def xgroup_delconsumer(self, name, groupname, consumername):
        self.deleted_consumers.append((name, groupname, consumername))
        return 1


MID = "11111111-1111-1111-1111-111111111111"


@pytest.mark.unit
def test_key_builders_are_machine_namespaced():
    assert machine_outbox.stream_key(MID) == f"machine_stream:{MID}"
    assert machine_outbox.group_name(MID) == f"machine-group:{MID}"
    assert machine_outbox.offline_stream_key(MID) == f"machine_offline_stream:{MID}"
    assert machine_outbox.consumer_name("s") == "machine-consumer-s"
    # Must not collide with the per-runner outbox namespace.
    from karma_team.runner.services import outbox

    assert machine_outbox.stream_key(MID) != outbox.stream_key(MID)


@pytest.mark.unit
def test_enqueue_online_goes_to_live_stream(monkeypatch):
    client = _FakeRedis()
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)
    monkeypatch.setattr(
        machine_outbox, "active_session_id_for_machine", lambda _mid: "sess-1"
    )

    sid = machine_outbox.enqueue_for_machine(MID, {"type": "create_runner", "name": "r1"})

    assert sid == "1-0"
    assert machine_outbox.stream_key(MID) in client.streams
    assert machine_outbox.offline_stream_key(MID) not in client.streams


@pytest.mark.unit
def test_enqueue_offline_queueable_type_buffers(monkeypatch):
    client = _FakeRedis()
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)
    monkeypatch.setattr(
        machine_outbox, "active_session_id_for_machine", lambda _mid: None
    )

    # ``welcome`` is not in the offline-reject set → it buffers.
    result = machine_outbox.enqueue_for_machine(MID, {"type": "welcome"})

    assert result is None
    assert machine_outbox.offline_stream_key(MID) in client.streams
    assert machine_outbox.stream_key(MID) not in client.streams


@pytest.mark.unit
def test_enqueue_offline_remove_runner_buffers_for_reconnect(monkeypatch):
    client = _FakeRedis()
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)
    monkeypatch.setattr(
        machine_outbox, "active_session_id_for_machine", lambda _mid: None
    )
    message = {
        "type": "remove_runner",
        "runner_id": "22222222-2222-2222-2222-222222222222",
        "mid": "project-delete:p:r",
    }

    result = machine_outbox.enqueue_for_machine(MID, message)

    assert result is None
    entries = client.streams[machine_outbox.offline_stream_key(MID)]
    assert len(entries) == 1
    assert entries[0][1]["type"] == "remove_runner"
    assert entries[0][1]["mid"] == "project-delete:p:r"


@pytest.mark.unit
def test_enqueue_offline_create_runner_raises(monkeypatch):
    client = _FakeRedis()
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)
    monkeypatch.setattr(
        machine_outbox, "active_session_id_for_machine", lambda _mid: None
    )

    with pytest.raises(MachineOfflineError) as exc:
        machine_outbox.enqueue_for_machine(MID, {"type": "create_runner"})
    assert exc.value.dev_machine_id == MID
    assert exc.value.message_type == "create_runner"
    # Nothing written to either stream on reject.
    assert client.streams == {}


@pytest.mark.unit
def test_enqueue_unknown_type_rejected(monkeypatch):
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: _FakeRedis())
    with pytest.raises(ValueError, match="unknown machine message type"):
        machine_outbox.enqueue_for_machine(MID, {"type": "assign"})


@pytest.mark.unit
def test_ack_for_session_calls_xack(monkeypatch):
    client = _FakeRedis()
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)
    n = machine_outbox.ack_for_session(MID, ["1-0", "2-0", ""])
    assert n == 2
    assert client.acked == ["1-0", "2-0"]


@pytest.mark.unit
def test_claim_pending_without_prior_session_reclaims_orphaned_pel(monkeypatch):
    client = _ClaimRedis(results=[(b"0-0", [[b"1-0", {b"payload": b"{}"}]])])
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)

    claimed = machine_outbox.claim_pending_for_new_session(
        MID,
        old_consumer=None,
        new_consumer="machine-consumer-new",
    )

    assert claimed == 1
    assert client.deleted_consumers == []


@pytest.mark.unit
def test_claim_pending_scans_empty_nonterminal_pages_and_cleans_old_consumer(
    monkeypatch,
):
    client = _ClaimRedis(
        results=[
            (b"250-0", []),
            (b"0-0", [[b"300-0", {b"payload": b"{}"}]]),
        ]
    )
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)

    claimed = machine_outbox.claim_pending_for_new_session(
        MID,
        old_consumer="machine-consumer-old",
        new_consumer="machine-consumer-new",
    )

    assert claimed == 1
    assert client.deleted_consumers == [
        (
            machine_outbox.stream_key(MID),
            machine_outbox.group_name(MID),
            "machine-consumer-old",
        )
    ]
    assert all("justid" not in call for call in client.claim_calls)


@pytest.mark.unit
def test_claim_pending_failure_keeps_old_consumer(monkeypatch):
    client = _ClaimRedis(error=RuntimeError("redis unavailable"))
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)

    claimed = machine_outbox.claim_pending_for_new_session(
        MID,
        old_consumer="machine-consumer-old",
        new_consumer="machine-consumer-new",
    )

    assert claimed == 0
    assert client.deleted_consumers == []


@pytest.mark.unit
def test_claim_pending_bails_when_cursor_does_not_advance(monkeypatch):
    client = _ClaimRedis(
        results=[
            (b"5-0", [[b"1-0", {b"payload": b"{}"}]]),
            (b"5-0", [[b"2-0", {b"payload": b"{}"}]]),
            (b"5-0", [[b"3-0", {b"payload": b"{}"}]]),
        ]
    )
    monkeypatch.setattr(machine_outbox, "redis_instance", lambda: client)

    claimed = machine_outbox.claim_pending_for_new_session(
        MID,
        old_consumer="machine-consumer-old",
        new_consumer="machine-consumer-new",
    )

    assert claimed == 2
    assert len(client.results) == 1
    assert client.deleted_consumers == []
