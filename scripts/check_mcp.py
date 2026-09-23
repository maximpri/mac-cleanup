#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise `diskray mcp` over stdio like an MCP client would.

Checks both protocol eras, tool listing, calls against a disposable fixture,
path escapes, oversized input, and that stdout carries only JSON-RPC.
"""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/diskray").resolve()
MODERN = "2026-07-28"
META = "io.modelcontextprotocol/protocolVersion"


class Client:
    def __init__(self, root):
        self.process = subprocess.Popen([str(binary), "mcp", "--root", str(root)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, bufsize=0)
        self.buffer = b""
        self.next_id = 0

    def send(self, message):
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def send_raw(self, data):
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def read(self, timeout=90):
        deadline = time.monotonic() + timeout
        descriptor = self.process.stdout.fileno()
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0, "no reply in time"
            ready, _, _ = select.select([descriptor], [], [], remaining)
            assert ready, "no reply in time"
            chunk = os.read(descriptor, 65536)
            assert chunk, "server closed its output"
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        message = json.loads(line)  # every stdout line must be JSON
        assert message["jsonrpc"] == "2.0"
        return message

    def request(self, method, params=None, modern=False):
        self.next_id += 1
        params = dict(params or {})
        if modern:
            params["_meta"] = {META: MODERN}
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        reply = self.read()
        assert reply["id"] == self.next_id, reply
        return reply

    def call(self, name, arguments, retries=4):
        for _ in range(retries):
            reply = self.request("tools/call", {"name": name, "arguments": arguments})
            text = reply["result"]["content"][0]["text"]
            if "still measuring" not in text:
                return reply["result"]
            time.sleep(5)
        raise AssertionError(f"{name}: assessment never finished")

    def close(self):
        self.process.stdin.close()
        return self.process.wait(timeout=10)


with tempfile.TemporaryDirectory(prefix="diskray-mcp-") as temporary:
    root = Path(temporary).resolve()
    (root / "Projects" / "app").mkdir(parents=True)
    (root / "Projects" / "app" / "data.bin").write_bytes(b"x" * 300_000)
    (root / ".Trash").mkdir()
    (root / ".Trash" / "old.bin").write_bytes(b"y" * 200_000)
    (root / "etc-link").symlink_to("/etc")

    client = Client(root)
    legacy = client.request("initialize", {"protocolVersion": "2025-11-25",
                                           "clientInfo": {"name": "check_mcp"}, "capabilities": {}})
    assert legacy["result"]["protocolVersion"] == "2025-11-25"
    assert legacy["result"]["serverInfo"]["name"] == "diskray"
    client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    assert client.request("ping")["result"] == {}
    print("PASS: legacy initialize, silent notification, ping")

    discover = client.request("server/discover", modern=True)["result"]
    assert discover["resultType"] == "complete" and MODERN in discover["supportedVersions"]
    assert discover["cacheScope"] == "private" and discover["ttlMs"] > 0
    client.send({"jsonrpc": "2.0", "id": 900, "method": "tools/list",
                 "params": {"_meta": {META: "1900-01-01"}}})
    unsupported = client.read()
    assert unsupported["error"]["code"] == -32022
    assert MODERN in unsupported["error"]["data"]["supported"]
    print("PASS: modern server/discover and unsupported-version error")

    tools = client.request("tools/list", modern=True)["result"]
    names = [tool["name"] for tool in tools["tools"]]
    assert names[0] == "disk_overview" and "propose_cleanup" in names
    assert names == [tool["name"] for tool in client.request("tools/list")["result"]["tools"]]
    print(f"PASS: {len(names)} tools in a stable order")

    overview = client.call("disk_overview", {})
    assert not overview["isError"] and "structuredContent" in overview
    listing = client.call("list_children", {"path": str(root)})
    assert not listing["isError"] and "Projects" in listing["content"][0]["text"], listing
    print("PASS: disk_overview and list_children on the fixture")

    artifacts = client.call("stale_artifacts", {"min_age_days": 30})
    assert not artifacts["isError"] and artifacts["structuredContent"]["deleted"] is False
    print("PASS: stale_artifacts reports without needing the assessment")

    for escape in [str(root / ".." / ".."), "/etc", str(root / "etc-link"), "relative"]:
        result = client.call("list_children", {"path": escape})
        assert result["isError"], (escape, result)
    print("PASS: path escapes are refused")

    client.send_raw(b"x" * (1024 * 1024 + 100) + b"\n")
    assert client.read()["error"]["code"] == -32600
    assert client.request("nope")["error"]["code"] == -32601
    assert client.request("ping")["result"] == {}
    print("PASS: oversized and unknown requests get errors; the server keeps serving")

    assert (root / ".Trash" / "old.bin").exists()
    assert client.close() == 0
    print("PASS: nothing in the fixture was changed; clean exit on EOF")
