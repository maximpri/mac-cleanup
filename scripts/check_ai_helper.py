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
helper = Path(os.environ.get("DISKRAY_AI_HELPER", Path(__file__).resolve().parents[1] / "target/release/diskray-ai"))


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
diagnostics = availability["diagnostics"]
assert isinstance(diagnostics["locale_supported"], bool)
assert diagnostics["device_language"] and diagnostics["context_size"] >= 0
assert isinstance(diagnostics["supported_languages"], list)
assert all(isinstance(tag, str) for tag in diagnostics["supported_languages"])
if availability["available"]:
    assert diagnostics["context_size"] > 0
    capabilities = availability["capabilities"]
    assert capabilities["provider"].startswith("Apple Foundation Models")
    assert capabilities["dynamic_schemas"] is True and capabilities["tool_calling"] is True
else:
    print(f"INFO: model unavailable ({availability['error_code']}); live checks need Apple Intelligence")
print("PASS: version rejection and structured availability")

# Shape/transport tests run even when model assets are unavailable.
sample_report = {"answer_kind": "clarification", "target_id": "n1", "evidence_ids": ["E1"],
                 "suggested_actions": ["none"], "phase": "inconclusive",
                 "clarification_question": "Keep using Cursor or remove its data?"}
response = request({"protocol": PROTOCOL, "request_id": "report-shape", "operation": "report_selftest",
                    "prompt_version": "scope-test-v1", "prompt": json.dumps(sample_report),
                    "allowed_targets": ["n1"], "allowed_evidence": ["E1"], "allowed_actions": []})
assert response["prompt_version"] == "scope-test-v1"
assert response["report"] == {**sample_report, "suggested_actions": [], "verdicts": []}
properties = response["schema"]["properties"]
assert set(properties["answer_kind"]["enum"]) == {"findings", "clarification", "insufficient_evidence"}
assert properties["target_id"]["enum"] == ["n1"]
assert properties["evidence_ids"]["items"]["enum"] == ["E1"]
assert properties["suggested_actions"]["items"]["enum"] == ["none"]
assert "clarification_question" not in response["schema"]["required"]
response = request({"protocol": PROTOCOL, "request_id": "report-actions", "operation": "report_selftest",
                    "allowed_targets": ["n1", "n2"], "allowed_actions": ["A1", "A3"]})
assert response["schema"]["properties"]["suggested_actions"]["items"]["enum"] == ["A1", "A3"]
response = request({"protocol": PROTOCOL, "request_id": "report-legacy", "operation": "report_selftest"})
assert response["schema"]["properties"]["target_id"]["enum"] == ["none"]
assert "enum" not in response["schema"]["properties"]["suggested_actions"]["items"]
assert "enum" not in response["schema"]["properties"]["evidence_ids"]["items"]
assert response["report"]["answer_kind"] == "insufficient_evidence"
assert "clarification_question" not in response["report"]
print("PASS: scoped report schema, optional request fields, and report transport without inference")

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

    limitation = {"task": "What grew since last week?", "tool_calls_left": 0,
                  "evidence": [{"id": "E1", "status": "unsupported",
                                "summary": "No comparable complete assessment at least 7 days old is saved."}]}
    response = request({"protocol": PROTOCOL, "request_id": "report-limitation", "operation": "report",
                        "prompt": json.dumps(limitation), "allowed_evidence": ["E1"],
                        "allowed_targets": ["none"], "allowed_actions": [], "prompt_version": "scope-test-v1",
                        "instructions": "Select collected evidence IDs. Cite unsupported checks to explain limitations, without claiming growth."})
    assert response["report"]["evidence_ids"] == ["E1"], response
    assert response["report"]["phase"] == "inconclusive"
    assert response["report"]["answer_kind"] == "insufficient_evidence", response
    assert response["report"]["target_id"] == "none"
    assert response["report"]["suggested_actions"] == []
    assert response["prompt_version"] == "scope-test-v1"
    print("PASS: live report cites missing history without claiming growth")

    scoped = {"task": "Can I clean old Cursor files?", "target": "n1 Cursor",
              "evidence": [{"id": "E1", "summary": "Cursor has app-managed settings and workspace data."}],
              "clarification": "Keep using Cursor, or remove its data?",
              "unrelated_action": "A9 clears pip; this action is outside the question scope."}
    response = request({"protocol": PROTOCOL, "request_id": "report-scope", "operation": "report",
                        "prompt": json.dumps(scoped), "allowed_evidence": ["E1"],
                        "allowed_targets": ["n1"], "allowed_actions": [],
                        "instructions": "The target is Cursor. Ask the supplied clarification; do not suggest unrelated cleanup."})
    assert response["report"]["answer_kind"] == "clarification", response
    assert response["report"]["target_id"] == "n1"
    assert response["report"]["suggested_actions"] == []
    print("PASS: scoped Cursor clarification cannot suggest unrelated pip actions")

    if capabilities["token_counting"] and diagnostics["context_size"] <= 12000:
        response = request({"protocol": PROTOCOL, "request_id": "report-context", "operation": "report",
                            "prompt": "Cite E1.", "instructions": "x " * diagnostics["context_size"],
                            "allowed_evidence": ["E1"], "allowed_actions": []})
        assert response.get("error_code") == "context", response
        assert response.get("type") == "error"
        print("PASS: final report counts instructions and schema against the context budget")

    relocation = {"category": "personal_data", "allocated_kb": 1048576,
                  "recently_modified": False,
                  "destinations": [{"id": "v1", "free_kb": 100000000, "capacity_kb": 200000000}]}
    response = request({"protocol": PROTOCOL, "request_id": "relocation-live", "operation": "relocation",
                        "prompt": json.dumps(relocation),
                        "allowed_destinations": ["v1", "keep_local", "inspect_first"]})
    assert response["relocation"]["choice"] in {"v1", "keep_local", "inspect_first"}
    print("PASS: live on-device relocation advice uses only permitted choices")

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
    assert used, "model must actually use at least one read-only tool"
    assert report["evidence_ids"]
    issued_evidence = {"E1"} | {answers[name].split(" ", 1)[0] for name in used}
    assert set(report["evidence_ids"]) <= issued_evidence
    assert set(report["suggested_actions"]) <= {"A1"}
    assert report["phase"] in {"complete", "inconclusive"}
    assert report["answer_kind"] in {"findings", "clarification", "insufficient_evidence"}
    assert report["target_id"] == "none"
    print(f"PASS: live tool-using investigation · tools {used} · cited {report['evidence_ids']}"
          f" · suggested {report['suggested_actions']} ({time.monotonic() - began:.2f}s)")
