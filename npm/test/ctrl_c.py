"""Presses Ctrl-C (or Ctrl-\\) at a terminal running a command, as a person would.

    python3 npm/test/ctrl_c.py [--quit] READY_TEXT COMMAND [ARG...]

Runs COMMAND in a new pseudo-terminal, waits until its output shows
READY_TEXT, types Ctrl-C once, and waits for it to end. Prints the output,
then exits 0 if COMMAND exited 0 and 1 otherwise. With --quit it types
Ctrl-\\ (SIGQUIT) instead and exits 0 once COMMAND has ended, however it
ended; a process it left behind on the terminal keeps this waiting until it
times out. The terminal signals every process in its foreground process
group, so this checks what a real keypress does under `npm run` and `npx`,
where npm also passes the signal on.
"""

import os
import pty
import select
import sys
import time


def main():
    args = sys.argv[1:]
    quit_key = args[0] == "--quit"
    if quit_key:
        args = args[1:]
    ready = args[0].encode()
    argv = args[1:]
    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(argv[0], argv)

    out = b""
    pressed = False
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if not pressed and ready in out:
            os.write(fd, b"\x1c" if quit_key else b"\x03")
            pressed = True
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
    else:
        os.kill(pid, 9)
        sys.stdout.write(out.decode(errors="replace"))
        print(f"ctrl_c: timed out (Ctrl-C pressed: {pressed})")
        return 1

    _, status = os.waitpid(pid, 0)
    sys.stdout.write(out.decode(errors="replace"))
    if os.WIFSIGNALED(status):
        print(f"ctrl_c: ended by signal {os.WTERMSIG(status)}")
        return 0 if quit_key and pressed else 1
    code = os.WEXITSTATUS(status)
    print(f"ctrl_c: exit {code}")
    if quit_key:
        return 0 if pressed else 1
    return 0 if code == 0 and pressed else 1


if __name__ == "__main__":
    sys.exit(main())
