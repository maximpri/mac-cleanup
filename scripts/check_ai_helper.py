#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check helper framing and the tool bridge without model assets.

`--live` additionally runs synthetic inference, including a tool-using
investigation answered by this script, and prints token budgets.
"""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time

PROTOCOL = 3
helper = Path(__file__).resolve().parents[1] / "target/release/diskray-ai"


def request(payload):
    result = subprocess.run([str(helper)], input=json.dumps(payload) + "\n",
                            capture_output=True, text=True, timeout=60, check=True)
    response = json.loads(result.stdout)
    assert response["protocol"] == PROTOCOL
    assert response["request_id"] == payload.get("request_id", "invalid")
    assert isinstance(response["available"], bool)
    return response


class Session:
    """A long-lived helper answered line by line, like the app's broker.

    Output is read from the raw descriptor with our own line buffer: several
    lines can arrive in one chunk, and a buffered reader would hide them from
    select().
    """

    def __init__(self, payload):
        self.process = subprocess.Popen([str(helper)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, bufsize=0)
        self.buffer = b""
        self.send(payload)

    def send(self, value):
        self.process.stdin.write((json.dumps(value) + "\n").encode())
        self.process.stdin.flush()

    def read(self, timeout=60):
        deadline = time.monotonic() + timeout
        descriptor = self.process.stdout.fileno()
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0, "helper produced no line in time"
            ready, _, _ = select.select([descriptor], [], [], remaining)
            assert ready, "helper produced no line in time"
            chunk = os.read(descriptor, 65536)
            assert chunk, "helper closed its output"
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        message = json.loads(line)
        assert message["protocol"] == PROTOCOL
        return message

    def close(self):
        self.process.stdin.close()
        return self.process.wait(timeout=5)


def tool(name, argument=None):
    spec = {"name": name, "description": f"Synthetic {name} for framing checks."}
    if argument:
        spec["argument"] = argument
    return spec


HANDLE = {"name": "folder", "description": "A folder handle such as n1.", "kind": "handle",
          "pattern": "n[0-9]{1,2}"}

invalid = request({"protocol": 999, "request_id": "version-test", "operation": "capabilities"})
assert not invalid["available"] and invalid["error"]
availability = request({"protocol": PROTOCOL, "request_id": "capabilities", "operation": "capabilities"})
assert availability["available"] or availability.get("error_code")
if availability["available"]:
    capabilities = availability["capabilities"]
    assert capabilities["provider"].startswith("Apple Foundation Models")
    assert capabilities["dynamic_schemas"] is True and capabilities["tool_calling"] is True
else:
    print(f"INFO: model unavailable ({availability['error_code']}); live checks need Apple Intelligence")
print("PASS: version rejection and structured availability")

# Concurrent calls: every tool call must be emitted before any reply is sent,
# and replies delivered out of order must reach the right caller.
names = ["alpha", "beta", "gamma"]
session = Session({"protocol": PROTOCOL, "request_id": "self", "operation": "selftest",
                   "tools": [tool(name, HANDLE if name == "alpha" else None) for name in names]})
calls = [session.read() for _ in names]
assert all(call["type"] == "tool_call" and call["request_id"] == "self" for call in calls)
assert sorted(call["tool"] for call in calls) == names
assert len({call["call_id"] for call in calls}) == len(names)
for call in reversed(calls):
    session.send({"type": "tool_result", "call_id": call["call_id"], "output": f"answer:{call['tool']}"})
final = session.read()
assert final["type"] == "final"
assert final["outputs"] == {name: f"answer:{name}" for name in names}
assert session.process.wait(timeout=5) == 0
print("PASS: concurrent tool calls, out-of-order replies, and routing")

session = Session({"protocol": PROTOCOL, "request_id": "cap", "operation": "selftest", "budget": 1,
                   "tools": [tool("first"), tool("second")]})
call = session.read()
session.send({"type": "tool_result", "call_id": call["call_id"], "output": "ok"})
final = session.read()
assert sorted(final["outputs"].values()) == ["budget", "ok"]
session.close()
print("PASS: hard call cap refuses calls beyond the budget")

session = Session({"protocol": PROTOCOL, "request_id": "eof", "operation": "selftest",
                   "tools": [tool("waiting")]})
assert session.read()["type"] == "tool_call"
began = time.monotonic()
assert session.close() == 0
assert time.monotonic() - began < 3
print("PASS: closing input cancels a waiting session promptly")

if "--live" in sys.argv:
    assert availability["available"], availability.get("error_code")
    evidence = {"revision": 1, "investigation": False, "subjects": [{
        "id": "item1", "title": "Python cache",
        "observation": "A rebuildable package cache was measured and is eligible for review.",
        "consequence": "Packages download again when needed.", "action_ids": [], "quick_win": True,
        "priority": 1, "disruption": "routine"}, {
        "id": "item2", "title": "Large folder", "observation": "A large folder needs review.",
        "consequence": "Useful data may be retained or moved.", "action_ids": [],
        "quick_win": False, "priority": 2, "disruption": "review"}]}
    began = time.monotonic()
    response = request({"protocol": PROTOCOL, "request_id": "triage-live", "operation": "triage",
                        "prompt": json.dumps(evidence), "allowed_key_areas": ["item2"],
                        "allowed_quick_wins": ["item1"]})
    triage = response["triage"]
    assert set(triage["quick_win_ids"]) <= {"item1"} and set(triage["key_area_ids"]) <= {"item2"}
    print(f"PASS: live synthetic triage references ({time.monotonic() - began:.2f}s)")

    tools = [
        tool("list_children", HANDLE),
        tool("folder_age", HANDLE),
        tool("open_handles", HANDLE),
        tool("growth_history", HANDLE),
        tool("cleanup_rule", HANDLE),
        tool("identify_owner", HANDLE),
    ]
    answers = {
        "list_children": "E2 · ~/Library/Caches/pip (1.0 GB, complete): n2 http 720 MB 70% · n3 wheels 304 MB 30%",
        "folder_age": "E3 · n1: 92% of 1.0 GB untouched for over 90 days; newest change 41 days ago.",
        "open_handles": "E4 · No process has files open under n1.",
        "growth_history": "E5 · No comparable complete history yet.",
        "cleanup_rule": "E6 · n1 matches the pip cache rule: READY. Packages download again when needed. A1 clears it.",
        "identify_owner": "E7 · Likely owner: pip (Python package installer). Heuristic from the path.",
    }
    packet = {"task": "Explain what uses this space and whether the listed action is appropriate.",
              "subject": "n1 ~/Library/Caches/pip · 1.0 GB · cache READY",
              "hypotheses": [{"id": "rebuildable_data", "claim": "The space is rebuildable cache data"},
                             {"id": "active_writer", "claim": "An active workload still uses this data"}],
              "evidence": [{"id": "E1", "status": "complete", "summary": "Initial measurement: 1.0 GB."}],
              "actions": [{"id": "A1", "action": "Clear pip cache (1.0 GB)"}], "tool_calls_left": 6}
    base = {"protocol": PROTOCOL, "prompt": json.dumps(packet), "tools": tools, "budget": 6,
            "allowed_hypotheses": ["rebuildable_data", "active_writer"]}
    measured = request({**base, "request_id": "measure", "operation": "measure"})
    print(f"INFO: setup tokens {measured['tokens']} of context {measured['context_size']}"
          f" ({'exact' if measured['exact'] else 'estimated'})")
    assert measured["tokens"]["total"] + 1600 <= measured["context_size"]
    began = time.monotonic()
    session = Session({**base, "request_id": "agent-live", "operation": "agent"})
    used = []
    while True:
        message = session.read(timeout=120)
        if message["type"] == "tool_call":
            used.append(message["tool"])
            output = answers.get(message["tool"], "Unsupported tool.")
            session.send({"type": "tool_result", "call_id": message["call_id"], "output": output})
            continue
        assert message["type"] == "final", message
        report = message["report"]
        break
    session.close()
    assert report["summary"].strip()
    assert set(report["suggested_actions"]) <= {"A1"}
    assert report["phase"] in {"complete", "inconclusive"}
    print(f"PASS: live tool-using investigation · tools {used} · cited {report['evidence_ids']}"
          f" · suggested {report['suggested_actions']} ({time.monotonic() - began:.2f}s)")
