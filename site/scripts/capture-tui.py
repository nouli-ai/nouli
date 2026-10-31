#!/usr/bin/env python3
"""Capture real PTY screens. Recording-only dependency: pyte; no live provider."""

import codecs, fcntl, json, os, pty, select, signal, struct, subprocess, sys, termios, time
import pyte

spec = json.load(sys.stdin)
binary = spec["nouli"]
home = spec["home"]
config = os.path.join(home, ".nouli", "config.toml")


def cli(*args):
    result = subprocess.run(
        [binary, "--config", config, *args, "--json"],
        cwd=home,
        env=dict(os.environ, HOME=home),
        capture_output=True,
        text=True,
    )
    if result.returncode:
        raise RuntimeError(result.stderr or result.stdout)
    return json.loads(result.stdout)


def frontier(name):
    return cli("sessions", "export", name)["session"]["frontier"]


class Terminal:
    def __init__(self, name):
        self.pid, self.fd = pty.fork()
        if not self.pid:
            os.chdir(home)
            env = dict(
                os.environ, HOME=home, TERM="xterm-256color", COLORTERM="truecolor"
            )
            env.pop("NO_COLOR", None)
            os.execve(binary, [binary, "--config", config, "open", name], env)
        self.width = 140
        self.height = 34
        fcntl.ioctl(
            self.fd,
            termios.TIOCSWINSZ,
            struct.pack("HHHH", self.height, self.width, 0, 0),
        )
        self.screen = pyte.Screen(self.width, self.height)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.wait(lambda: "Ready" in self.text())

    def text(self):
        return "\n".join(
            "".join(self.screen.buffer[y][x].data for x in range(self.width))
            for y in range(self.height)
        )

    def pump(self, duration=0.08):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select(
                [self.fd], [], [], max(0, end - time.monotonic())
            )
            if not ready:
                return
            data = os.read(self.fd, 65536)
            if not data:
                raise RuntimeError("TUI exited")
            self.stream.feed(self.decoder.decode(data))

    def wait(self, predicate, seconds=15):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.pump()
            if predicate():
                return
        raise RuntimeError(self.text())

    def send(self, data):
        os.write(self.fd, data)
        self.pump()

    def paste(self, text):
        self.send(b"\x1b[200~" + text.encode() + b"\x1b[201~")

    def command(self, command):
        self.paste(command)
        self.send(b"\r")

    def resize(self, width):
        self.width = width
        self.screen.resize(self.height, width)
        fcntl.ioctl(
            self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", self.height, width, 0, 0)
        )
        os.kill(self.pid, signal.SIGWINCH)
        self.pump(0.15)

    def capture(self):
        rows = []
        for y in range(self.height):
            cells = [self.screen.buffer[y][x] for x in range(self.width)]
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
        return {"columns": self.width, "rows": rows}

    def close(self):
        for _ in range(7):
            try:
                self.send(b"\x03")
            except (OSError, RuntimeError):
                pass
            done, status = os.waitpid(self.pid, os.WNOHANG)
            if done:
                os.close(self.fd)
                if os.waitstatus_to_exitcode(status):
                    raise RuntimeError("TUI exit failed")
                return
        os.kill(self.pid, signal.SIGTERM)
        os.waitpid(self.pid, 0)
        raise RuntimeError("TUI did not quit")


spaces = {}
frames = []
current = spec["steps"][0]["session"]
kind = "noul"
terminal = Terminal(current)
try:
    for index, step in enumerate(spec["steps"]):
        if step["verb"] == "whatif":
            subprocess.run(
                [binary, "--config", config, *step["args"]],
                cwd=home,
                env=dict(os.environ, HOME=home),
                check=True,
                capture_output=True,
            )
            continue
        if step["verb"] == "new" and index:
            if len(step["from"]) > 1:
                terminal.command(
                    "/blend " + " ".join(step["from"]) + " as " + step["session"]
                )
            elif step["from"]:
                if current != step["from"][0]:
                    terminal.command("/use " + step["from"][0])
                terminal.command("/branch " + step["session"])
            else:
                terminal.command("/new " + step["session"])
            current = step["session"]
        elif current != step["session"]:
            terminal.command("/use " + step["session"])
            current = step["session"]
        q = step["question"]
        if not q:
            raise RuntimeError("Saved step needs one recorded question")
        kind = next(
            (
                value
                for label, value in [
                    ("[Yes / No]", "noul"),
                    ("[Choose one]", "choice"),
                    ("[Score]", "score"),
                ]
                if label in terminal.text()
            ),
            None,
        )
        if kind is None:
            raise RuntimeError("Cannot identify answer type: " + terminal.text())
        original_kind = kind
        while kind != q["type"]:
            terminal.send(b"\x1b[Z")
            kind = {"noul": "choice", "choice": "score", "score": "noul"}[kind]
        if step["facts"]:
            terminal.paste("\n".join(step["facts"]))
        terminal.send(b"\t\x15")
        terminal.paste(q["prompt"])
        labels = [option["label"] for option in q.get("options", q.get("levels", []))]
        reuse = (
            labels
            and original_kind == q["type"]
            and spaces.get((current, q["type"])) == labels
        )
        if labels and not reuse:
            terminal.send(b"\t\x15")
            terminal.paste("\n".join(labels))
        draft = terminal.capture()
        terminal.resize(80)
        draft_narrow = terminal.capture()
        terminal.resize(140)
        before = frontier(current)
        terminal.send(b"\x13")
        terminal.wait(
            lambda: "Ready" in terminal.text() and frontier(current) != before
        )
        answer = terminal.capture()
        terminal.resize(80)
        narrow = terminal.capture()
        terminal.send(b"\x07")
        graph = terminal.capture()
        terminal.send(b"\x1b")
        terminal.resize(140)
        spaces[(current, q["type"])] = labels
        frames.append(
            {
                "step": index,
                "title": q["prompt"],
                "kind": q["type"],
                "session": current,
                "draft": draft,
                "draftNarrow": draft_narrow,
                "answer": answer,
                "narrow": narrow,
                "graph": graph,
            }
        )
        print("captured " + current + ": " + q["prompt"], file=sys.stderr)
finally:
    terminal.close()
print(
    json.dumps(
        {
            "version": 1,
            "application": subprocess.check_output(
                [binary, "--version"], text=True
            ).strip(),
            "replayed": True,
            "frames": frames,
        },
        ensure_ascii=False,
    )
)
