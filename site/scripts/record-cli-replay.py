"""Drive the full-screen Nouli app in a PTY and record every screen change.

Run by record-cli-replay.mjs, which supplies a stub provider that replays the
answers saved in src/data/cli/graph.json. Environment:

  NOULI_BIN      the nouli executable to record
  REPLAY_DIR     a private directory holding config.toml; the store and
                 recording.json are written there
  REPLAY_COLUMNS terminal width; wide terminals show the decision graph
                 beside the transcript, narrow ones only the transcript
"""

import codecs
import fcntl
import json
import os
import pty
import select
import signal
import struct
import subprocess
import termios
import time
from pathlib import Path

import pyte

BIN = os.environ["NOULI_BIN"]
DIR = Path(os.environ["REPLAY_DIR"])
STORE, CONFIG = DIR / "store", DIR / "config.toml"
WIDTH, HEIGHT = int(os.environ.get("REPLAY_COLUMNS", "160")), 38
# At this width and wider the app shows its decision graph beside the transcript.
GRAPH_BESIDE = 116

# The illustrative workshop: one yes/no, three choices and a score, with a
# branch, a second branch and a blend.
STEPS = [
    dict(session="workshop", kind="noul",
         facts="Plan a step-free workshop for 60 people. Budget: 6000 USD. Keep the event reliable and practical.",
         q="Can we plan this workshop?"),
    dict(session="venue", source=["workshop"],
         facts="Riverside Hall is step-free, fits 80 and costs 1200 USD. The Loft costs 900 USD but has stairs.",
         q="Which venue should we choose?", options=["Riverside Hall", "The Loft"]),
    dict(session="program", source=["workshop"],
         facts="Two speakers are available. Include breaks and useful hands-on time. The program costs 500 USD.",
         q="Which program should we run?", options=["Talks and practice", "Talks only"]),
    dict(session="plan", source=["venue", "program"],
         facts="The venue and speakers can confirm today. Total event costs are 3500 USD, including lunch, materials and transport.",
         q="Should we confirm the plan?", options=["Confirm the plan", "Revise the plan"]),
    dict(session="plan", kind="score",
         facts="The venue, speakers and lunch are confirmed. The morning bus is booked, but the return trip still has no driver.",
         q="How ready is the workshop?", options=["Not ready", "Needs work", "Mostly ready", "Ready"]),
]
NEXT_KIND = {"noul": "choice", "choice": "score", "score": "noul"}
KIND_LABELS = [("[Yes / No]", "noul"), ("[Choose one]", "choice"), ("[Score]", "score")]


def frontier(name):
    out = subprocess.run(
        [BIN, "--config", str(CONFIG), "--store", str(STORE), "sessions", "export", name, "--json"],
        capture_output=True, text=True, check=True,
    ).stdout
    return json.loads(out)["session"]["frontier"]


class Recorder:
    def __init__(self):
        self.screen = pyte.Screen(WIDTH, HEIGHT)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.frames, self.markers, self.previous = [], [], None
        self.started, self.last = time.monotonic(), -1.0
        self.pid, self.fd = pty.fork()
        if not self.pid:
            os.chdir(DIR)
            env = dict(os.environ, HOME=str(DIR), TERM="xterm-256color", COLORTERM="truecolor")
            env.pop("NO_COLOR", None)
            os.execve(BIN, [BIN, "--config", str(CONFIG), "--store", str(STORE), "open", "workshop"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", HEIGHT, WIDTH, 0, 0))
        self.wait(lambda: "Shift-Tab question type" in self.text())
        if WIDTH >= GRAPH_BESIDE:
            # Mouse reporting is enabled only while Ctrl-G focuses the graph.
            self.send(b"\x07")
            start, end = WIDTH - 64, WIDTH // 2
            self.send(f"\x1b[<0;{start + 1};3M".encode())
            self.send(f"\x1b[<32;{end + 1};3M".encode())
            self.send(f"\x1b[<0;{end + 1};3m".encode())
            self.send(b"\x1b")
        # The replay starts from the first ready screen.
        self.frames, self.previous = [], None
        self.snapshot(force=True)

    def now(self):
        return round(time.monotonic() - self.started, 3)

    def text(self):
        return "\n".join("".join(self.screen.buffer[y][x].data for x in range(WIDTH)) for y in range(HEIGHT))

    def snapshot(self, force=False):
        if not force and self.now() - self.last < 0.016:
            return
        rows = []
        for y in range(HEIGHT):
            cells = [self.screen.buffer[y][x] for x in range(WIDTH)]
            while cells and cells[-1].data == " ":
                cells.pop()
            runs = []
            for cell in cells:
                if not cell.data:
                    continue
                style = [cell.fg, cell.bg, cell.bold, cell.reverse]
                if runs and runs[-1][1:] == style:
                    runs[-1][0] += cell.data
                else:
                    runs.append([cell.data, *style])
            rows.append(runs)
        cursor = None if self.screen.cursor.hidden else [self.screen.cursor.x, self.screen.cursor.y]
        if [rows, cursor] != self.previous:
            self.frames.append(dict(t=self.now(), rows=rows, cursor=cursor))
            self.previous, self.last = [rows, cursor], self.now()

    def pump(self, duration=0.09):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.fd], [], [], max(0, end - time.monotonic()))
            if not ready:
                break
            data = os.read(self.fd, 65536)
            if not data:
                raise RuntimeError("TUI exited")
            self.stream.feed(self.decoder.decode(data))
            self.snapshot()
        self.snapshot()

    def wait(self, ready, seconds=90):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            self.pump(0.12)
            if ready():
                self.snapshot(force=True)
                return
            if "error:" in self.text().lower():
                raise RuntimeError("TUI reported an error:\n" + self.text())
        raise RuntimeError("Timed out waiting for the TUI")

    def send(self, data):
        os.write(self.fd, data)
        self.pump()

    def type(self, text):
        for character in text:
            os.write(self.fd, character.encode())
            self.pump(0.028)
        self.pump(0.08)

    def command(self, text):
        self.type(text)
        self.pump(0.18)
        self.send(b"\r")
        self.pump(0.25)

    def mark(self, index, phase, step):
        self.snapshot(force=True)
        self.markers.append(dict(t=self.now(), step=index, phase=phase, session=step["session"], title=step["q"]))

    def close(self):
        self.pump(0.5)
        self.snapshot(force=True)
        (DIR / "recording.json").write_text(json.dumps(
            dict(columns=WIDTH, lines=HEIGHT, frames=self.frames, markers=self.markers),
            ensure_ascii=False, separators=(",", ":")))
        os.write(self.fd, b"\x03")
        for _ in range(40):
            try:
                self.pump(0.06)
            except (OSError, RuntimeError):
                pass
            if os.waitpid(self.pid, os.WNOHANG)[0]:
                break
        else:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def main():
    recorder, current, kind = Recorder(), "workshop", "noul"
    try:
        for index, step in enumerate(STEPS):
            name, sources = step["session"], step.get("source", [])
            if len(sources) == 1:
                if current != sources[0]:
                    recorder.command("/use " + sources[0])
                recorder.command("/branch " + name)
            elif sources:
                recorder.command("/blend " + " ".join(sources) + " as " + name)
            elif name != current:
                recorder.command("/use " + name)
            current = name
            recorder.wait(lambda: "Shift-Tab question type" in recorder.text())
            # Opening a saved source can restore its answer type.
            heading = next(line for line in recorder.text().splitlines()
                           if line.lstrip("─ ").startswith("Question"))
            kind = "choice" if "Choose one" in heading else "score" if "Score" in heading else "noul"
            while kind != step.get("kind", "choice"):
                recorder.send(b"\x1b[Z")
                kind = NEXT_KIND[kind]
            recorder.type(step["facts"])
            recorder.send(b"\r")  # Queue the nonempty Context row.
            recorder.send(b"\r")  # Blank Enter advances to Question.
            recorder.type(step["q"])
            labels = step.get("options", [])
            if labels:
                recorder.send(b"\t\x15")
                for position, label in enumerate(labels):
                    recorder.type(label)
                    if position < len(labels) - 1:
                        recorder.send(b"\r")
            recorder.mark(index, "question", step)
            recorder.pump(0.35)
            before = frontier(current)
            recorder.send(b"\x13")
            recorder.wait(lambda: "Shift-Tab question type" in recorder.text() and frontier(current) != before)
            recorder.mark(index, "answer", step)
            recorder.pump(1.1)
            print(f"{index + 1}/{len(STEPS)} recorded: {step['q']}", flush=True)
        if WIDTH >= GRAPH_BESIDE:
            # Focus the decision graph from its top, so every decision is in view.
            # Narrow terminals would crop the graph, so they stay on the transcript.
            recorder.send(b"\x07")
            recorder.send(b"\x1b[H")
            recorder.pump(2.0)
            recorder.mark(len(STEPS) - 1, "graph", STEPS[-1])
        if os.environ.get("REPLAY_TOUR") == "1":
            recorder.send(b"\x1b")
            recorder.type("Keep this draft while choosing a model.")
            recorder.send(b"\x0c")  # Ctrl-L model picker.
            recorder.pump(4.0)
            recorder.send(b"\x1b")
            recorder.pump(1.0)
            recorder.send(b"\x15")
            recorder.command("/statusline")
            recorder.pump(4.0)
            recorder.send(b"\x1b")
            recorder.command("/help")
            recorder.pump(4.0)
            recorder.send(b"\x1b")
            if WIDTH >= GRAPH_BESIDE:
                recorder.send(b"\x07")
                start, end = WIDTH // 2, WIDTH // 2 + 10
                recorder.send(f"\x1b[<0;{start + 1};3M".encode())
                recorder.send(f"\x1b[<32;{end + 1};3M".encode())
                recorder.send(f"\x1b[<0;{end + 1};3m".encode())
                recorder.pump(2.0)
                recorder.send(b"\x1b")
            recorder.send(b"\x10")  # Ctrl-P holds the view.
            recorder.pump(3.0)
            recorder.send(b"\x1b")
        recorder.pump(1.0)
    finally:
        recorder.close()


if __name__ == "__main__":
    main()
