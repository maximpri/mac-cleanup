#!/usr/bin/env python3
"""Check helper framing without requiring model assets; --live uses synthetic evidence."""
import json
from pathlib import Path
import subprocess
import sys
import time

helper = Path(__file__).resolve().parents[1] / "target/release/mac-cleanup-ai"


def request(payload):
    result = subprocess.run([str(helper)], input=json.dumps(payload) + "\n",
                            capture_output=True, text=True, timeout=25, check=True)
    response = json.loads(result.stdout)
    assert response["protocol"] == 2
    assert response["request_id"] == payload.get("request_id", "invalid")
    assert isinstance(response["available"], bool)
    return response


invalid = request({"protocol": 999, "request_id": "version-test", "operation": "capabilities"})
assert not invalid["available"] and invalid["error"]
availability = request({"protocol": 2, "request_id": "capabilities", "operation": "capabilities"})
assert availability["available"] or availability.get("error")
if availability["available"]:
    capabilities = availability["capabilities"]
    assert capabilities["provider"].startswith("Apple Foundation Models")
    assert capabilities["dynamic_schemas"] is True
print("PASS: version rejection and structured availability")

if "--live" in sys.argv:
    assert availability["available"], availability.get("error")
    evidence = {"revision": 1, "investigation": False, "subjects": [{
        "id": "cache-1", "title": "Python cache",
        "observation": "A rebuildable package cache was measured and is eligible for review.",
        "consequence": "Packages download again when needed.",
        "action_ids": ["clean:cache-1"], "quick_win": True,
        "priority": 1, "disruption": "routine"}, {
        "id": "folder-1", "title": "Large folder", "observation": "A large folder needs review.",
        "consequence": "Useful data may be retained or moved.", "action_ids": [],
        "quick_win": False, "priority": 2, "disruption": "review"}]}
    began = time.monotonic()
    response = request({"protocol": 2, "request_id": "explain-live", "operation": "explain", "prompt": json.dumps(evidence),
                        "allowed_evidence": ["cache-1", "folder-1"], "allowed_actions": ["clean:cache-1"],
                        "allowed_checks": []})
    insight = response["insight"]
    assert insight["summary"].strip()
    assert insight["evidence_ids"] and set(insight["evidence_ids"]) <= {"cache-1", "folder-1"}
    assert set(insight["action_ids"]) <= {"clean:cache-1"}
    assert not insight["next_checks"]
    print(f"PASS: live synthetic explanation and references ({time.monotonic() - began:.2f}s)")
    began = time.monotonic()
    triage_evidence = json.loads(json.dumps(evidence))
    triage_evidence["subjects"][0]["id"] = "item1"
    triage_evidence["subjects"][1]["id"] = "item2"
    response = request({"protocol": 2, "request_id": "triage-live", "operation": "triage", "prompt": json.dumps(triage_evidence),
                        "allowed_key_areas": ["item2"], "allowed_quick_wins": ["item1"]})
    triage = response["triage"]
    assert set(triage["quick_win_ids"]) <= {"item1"}
    assert set(triage["key_area_ids"]) <= {"item2"}
    assert triage["reasons"] == []
    print(f"PASS: live synthetic triage references ({time.monotonic() - began:.2f}s)")
    began = time.monotonic()
    result_evidence = {"revision": 2, "investigation": False, "subjects": [{
        "id": "result:0", "title": "Completed maintenance action",
        "observation": "The reviewed action completed and the post-action measurement is available.",
        "consequence": "Compare the recorded before and after observations.",
        "action_ids": [], "quick_win": False, "priority": 2, "disruption": "completed"}]}
    response = request({"protocol": 2, "request_id": "result-live", "operation": "result", "prompt": json.dumps(result_evidence),
                        "allowed_evidence": ["result:0"], "allowed_actions": [], "allowed_checks": []})
    summary = response["insight"]
    assert summary["summary"].strip()
    assert summary["evidence_ids"] and set(summary["evidence_ids"]) <= {"result:0"}
    assert not summary["action_ids"] and not summary["next_checks"]
    print(f"PASS: live result summary references ({time.monotonic() - began:.2f}s)")
    began = time.monotonic()
    decision_evidence = {"case_id": "case-1", "revision": 1, "target": "fseventsd",
        "hypotheses": [{"id": "filesystem_activity", "label": "Filesystem activity"}],
        "evidence": [], "available_checks": [{"id": "fs_usage", "question": "Distinguish paging from file activity"}]}
    response = request({"protocol": 2, "request_id": "decision-live", "operation": "decide",
        "prompt": json.dumps(decision_evidence), "allowed_checks": ["fs_usage"],
        "allowed_evidence": [], "allowed_hypotheses": ["filesystem_activity"]})
    decision = response["decision"]
    assert decision["kind"] in {"check", "finish"}
    if decision["kind"] == "check":
        assert decision["check"] == "fs_usage"
    print(f"PASS: live constrained agent decision ({time.monotonic() - began:.2f}s)")
