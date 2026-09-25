#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the built macOS TUI. All deletion is confined to our disposable fixture."""

import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata

class Screen:
    """Small VT screen reader for the cursor/erase sequences emitted by Ratatui."""
    def __init__(self, width=120, height=30):
        self.resize(width, height)
        self.pending = ""
        self.cursor_reports = 0
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def resize(self, width, height):
        self.width, self.height = width, height
        self.rows = [[" "] * width for _ in range(height)]
        self.x = self.y = 0

    def feed(self, data):
        text = self.pending + self.decoder.decode(data)
        self.pending = ""
        i = 0
        while i < len(text):
            char = text[i]
            if char == "\x1b":
                if i + 1 >= len(text):
                    break
                if text[i + 1] == "[":
                    match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", text[i:])
                    if not match:
                        break
                    params, _, action = match.groups()
                    if not params.startswith("?"):
                        numbers = [int(n or 0) for n in params.split(";")] if params else [0]
                        n = numbers[0]
                        if action == "n" and n == 6:
                            self.cursor_reports += 1
                        if action in "Hf":
                            self.y = max(0, n - 1)
                            self.x = max(0, (numbers[1] if len(numbers) > 1 else 1) - 1)
                        elif action == "G": self.x = max(0, n - 1)
                        elif action == "d": self.y = max(0, n - 1)
                        elif action == "A": self.y = max(0, self.y - (n or 1))
                        elif action == "B": self.y += n or 1
                        elif action == "C": self.x += n or 1
                        elif action == "D": self.x = max(0, self.x - (n or 1))
                        elif action == "J" and n in (2, 3):
                            self.rows = [[" "] * self.width for _ in range(self.height)]
                        elif action == "K" and self.y < self.height:
                            begin, end = (0, self.width) if n == 2 else ((0, self.x + 1) if n == 1 else (self.x, self.width))
                            self.rows[self.y][begin:end] = [" "] * max(0, end - begin)
                    i += len(match.group())
                    continue
                i += 2
                continue
            if char == "\r": self.x = 0
            elif char == "\n": self.y += 1
            elif char == "\b": self.x = max(0, self.x - 1)
            elif char >= " ":
                if self.x >= self.width:
                    self.x = 0
                    self.y += 1
                if self.y < self.height:
                    self.rows[self.y][self.x] = char
                self.x += 0 if unicodedata.combining(char) else (2 if unicodedata.east_asian_width(char) in "WF" else 1)
            i += 1
        self.pending = text[i:]

    def text(self):
        return "\n".join("".join(row) for row in self.rows)


class Session:
    def __init__(self, binary, *args):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        self.process = subprocess.Popen(
            [str(binary), *args], stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, "TERM": "xterm-256color"}, start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(slave)
        self.screen = Screen()

    def feed(self, data):
        self.screen.feed(data)
        while self.screen.cursor_reports:
            os.write(self.master, b"\x1b[1;1R")
            self.screen.cursor_reports -= 1

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                self.feed(os.read(self.master, 262144))

    def wait_for(self, text, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            # Drain pending output before checking so the application is never
            # left with an unread full pty buffer while this harness decides
            # the screen already matches, and so consecutive keystrokes are
            # delivered as separate reads (ESC + key in one read parses as
            # Alt+key instead of two keys).
            if select.select([self.master], [], [], 0.05)[0]:
                data = os.read(self.master, 262144)
                self.feed(data)
            if re.sub(r"\s+", "", text) in re.sub(r"\s+", "", self.screen.text()):
                self.pump(0.12)  # Finish the frame before callers inspect its targets.
                return
            if self.process.poll() is not None:
                raise AssertionError(f"TUI exited before {text!r}: {self.process.returncode}")
        raise AssertionError(f"TUI did not display {text!r} within {timeout}s\n{self.screen.text()}")

    def send(self, keys):
        os.write(self.master, keys)
        time.sleep(0.05)

    def resize(self, width, height):
        self.screen.resize(width, height)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.process.send_signal(signal.SIGWINCH)

    def wait_exit(self, timeout=5):
        # A PTY has a bounded output buffer: drain it while the app restores
        # terminal state so waiting for exit cannot deadlock the writer.
        deadline = time.monotonic() + timeout
        while self.process.poll() is None and time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.1)[0]:
                try:
                    data = os.read(self.master, 262144)
                    if data:
                        self.feed(data)
                except OSError:
                    break
        return self.process.wait(timeout=max(0.1, deadline - time.monotonic()))

    def close(self):
        try:
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.wait_exit()
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
        finally:
            os.close(self.master)


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/diskray").resolve()
    with tempfile.TemporaryDirectory(prefix="diskray-smoke-") as temporary:
        root = Path(temporary).resolve()
        trash = root / ".Trash"
        trash.mkdir()
        candidate = trash / "disposable-fixture.bin"
        candidate.write_bytes(b"test fixture\n" * 4096)
        protected = root / "keep.txt"
        protected.write_text("outside the cleanup allowlist")
        report = subprocess.run([str(binary), "--analyze", "--json", "--volume", str(root)], capture_output=True, text=True, check=True, timeout=30)
        assert json.loads(report.stdout)["schema_version"] == 5
        assert candidate.exists()
        print("PASS: read-only JSON schema 5")
        session = Session(binary, "--analyze", "--volume", str(root))
        try:
            session.wait_for("Total used", timeout=45)
            # A real memory-pressure warning can precede storage findings.
            # Select the fixture explicitly before checking its folder map.
            session.wait_for("Volume Trash")
            for _ in range(12):
                if re.search(r"›\s+Volume Trash(?:\s|$)", session.screen.text()):
                    break
                session.send(b"\x1b[B")
                session.pump(0.2)
            else:
                raise AssertionError(f"fixture Trash was not selected\n{session.screen.text()}")
            session.wait_for("CONTENTS HEATMAP")
            session.wait_for("disposable-fixture.bin")
            session.send(b"\x1bOR")  # F3: auto-detected Apple model and readiness.
            session.wait_for("auto-detected")
            session.wait_for("Automatic")
            session.send(b"\x1b")
            session.wait_for("CONTENTS HEATMAP")
            session.send(b"v")
            session.wait_for("STORAGE BALANCE")
            session.wait_for("Outside this scan")
            session.send(b"\x1b")
            session.wait_for("WHERE")
            session.send(b":")
            session.wait_for("COMMANDS")
            session.send(b"history\r")
            session.wait_for("SAVED SCANS")
            session.send(b"g")
            session.wait_for("WHERE")
            session.send(b"\r")  # Open the selected finding directly.
            session.wait_for("FOLDERS")
            session.wait_for("disposable-fixture.bin")
            session.send(b"\r")  # A file opens details; Esc only leaves those details.
            session.wait_for("Tab to list")
            session.send(b"\x1b")
            session.wait_for("Tab to focus")
            session.send(b"v")
            session.wait_for("TOTALS & COVERAGE")
            session.send(b"\x1b[D")  # Left closes coverage before any folder navigation.
            session.wait_for("DETAILS")
            session.wait_for("FOLDERS")
            session.send(b"\x1b[D")  # Actual parent contains the sibling keep.txt.
            session.wait_for("keep.txt")
            session.send(b"\x1b")  # Back restores the folder we came from.
            session.wait_for("disposable-fixture.bin")
            session.send(b"\x1b")
            session.wait_for("WHERE")
            session.send(b"\r")
            session.wait_for("FOLDERS")
            session.send(b"t ")  # Neither Trash nor cleanup is selectable read-only.
            session.pump(0.2)
            session.send(b"p")
            session.wait_for("YOUR PLAN IS EMPTY")
            assert candidate.exists(), "browsing and read-only actions must preserve files"
            session.send(b"\x1b")
            session.wait_for("FOLDERS")
            session.send(b"g")
            session.wait_for("WHERE")
            session.send(b"b")
            session.wait_for("FOLDERS")
            session.send(b"\t")
            session.wait_for("Tab to list")
            session.send(b"\t")
            session.send(b"P")
            session.wait_for("LIVE PROCESSES")
            session.send(b"\x1b")
            session.wait_for("DISKRAY")
            session.resize(60, 16)
            session.wait_for("DISKRAY")
            session.send(b"?")
            session.wait_for("HELP")
            session.send(b" ")
            session.pump(0.15)
            assert candidate.exists(), "help must capture action keys"
            session.send(b"\x1b")
            session.wait_for("FOLDERS")
            session.send(b"h")
            session.wait_for("SAVED SCANS")
            session.send(b"\r")
            session.wait_for("RESULTS")
            session.send(b"\x1b")
            session.wait_for("SAVED SCANS")
            session.resize(45,12)
            session.wait_for("Expand the terminal")
            session.send(b"CLEAN\r")
            session.pump(0.8)  # Keep the window small until queued keys have been handled.
            assert candidate.exists()
            session.resize(120,30)
            session.wait_for("DISKRAY")
            time.sleep(0.25)
            session.send(b"q")
            assert session.wait_exit()==0
        finally:
            session.close()
        print("PASS: two panels, command palette, panel focus, process inspection, read-only resize, and exit")
        session = Session(binary, "--analyze", "--volume", str(root))
        try:
            session.wait_for("✓ Scan", timeout=45)
            session.wait_for("Ask AI ›")
            session.send(b"/")
            session.wait_for("Esc browse")
            session.send(b"q")
            session.wait_for("Ask AI › q")
            assert session.process.poll() is None, "q is text inside the Ask box"
            session.send(b"\x1b")
            session.wait_for("WHERE")
            assert "Ask AI › q" in session.screen.text(), "Esc must preserve the draft"
            session.send(b"/")
            session.wait_for("Esc browse")
            assert "Ask AI › q" in session.screen.text(), "refocusing must restore the draft"
            session.send(b"\x1b")
            session.pump(3.0)  # Let measured triage settle so the selection is stable.
            session.send(b"i")
            # i already focuses details. An extra Enter can clear the completion
            # message when a small fixture's investigation finishes immediately.
            session.wait_for("Investigation finished", timeout=90)
            seen = session.screen.text()
            for _ in range(12):  # PgDn: measurements come before interpretation.
                session.send(b"\x1b[6~")
                session.pump(0.15)
                seen += session.screen.text()
            assert any(label in seen for label in ("WHAT THE CHECKS FOUND", "LOCAL AI")), seen
            assert "tool calls" in seen and "HOW THIS WAS CHECKED" in seen, seen
            assert candidate.exists(), "investigations are read-only"
            session.send(b"\x1b")
            session.pump(0.2)
            session.send(b"q")
            assert session.wait_exit() == 0
        finally:
            session.close()
        print("PASS: ask box, read-only tool investigation timeline, and exit")
        session = Session(binary, "--volume", str(root))
        try:
            session.wait_for("✓ Scan",timeout=45)
            session.send(b"f")
            session.wait_for("f fewer")
            # Inspect each plan without executing until the exact fixture is selected.
            for _ in range(40):
                session.send(b" ")
                session.send(b"p")
                session.wait_for("Reviewing your plan")
                review_text = session.screen.text()
                # Exact paths wrap in the right panel. Rejoin its bordered
                # content without interleaving text from the persistent list.
                right_text = "".join(line.rsplit("│", 2)[-2].strip()
                                     for line in review_text.splitlines() if line.count("│") >= 4)
                if str(trash) in right_text:
                    break
                if "YOUR PLAN IS EMPTY" in review_text:
                    session.send(b"\x1b")
                else:
                    session.send(b"\x1b[3~")  # Delete clears this unexecuted plan.
                session.wait_for("f fewer")
                session.send(b"\x1b[B")
            else:
                raise AssertionError("fixture Trash was not selectable")
            assert "1 action(s)" in session.screen.text()
            session.send(b"\x1b")
            session.wait_for("f fewer")
            assert candidate.exists(), "cancel must preserve data"
            session.send(b"p")
            session.wait_for("Type CLEAN")
            session.send(b"CLEAN\r")
            session.wait_for("Plan finished",timeout=45)
            assert trash.is_dir()
            assert not candidate.exists()
            assert protected.read_text()=="outside the cleanup allowlist"
            time.sleep(6)
            assert session.process.poll() is None, "results must stay open"
            session.send(b"q")
            assert session.wait_exit()==0
        finally:
            session.close()
        print("PASS: exact-path review, cancellation, fixture-only cleanup, persistent results")


if __name__ == "__main__":
    main()
