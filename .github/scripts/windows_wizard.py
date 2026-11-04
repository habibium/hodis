"""Answers `hodis setup` in a Windows console, installing SearXNG.

Reads on a thread of its own, as a read blocks while hodis is quiet, so that a
hang fails at a deadline rather than at the job's timeout.
"""

import re
import sys
import threading
import time

from winpty import PtyProcess

ESCAPES = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07")

process = PtyProcess.spawn([sys.argv[1], "setup"], dimensions=(40, 120))
output = []


def pump():
    while True:
        try:
            output.append(process.read(4096))
        except EOFError:
            return


def text():
    return ESCAPES.sub("", "".join(output))


def wait_for(needle, seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if needle in text():
            return
        time.sleep(0.2)
    print(text())
    process.terminate(force=True)
    sys.exit(f"never saw {needle!r}")


threading.Thread(target=pump, daemon=True).start()

wait_for("Which search provider", 30)
process.write("\x1b[B")  # down to SearXNG, to prove arrows work
wait_for("> SearXNG", 10)
process.write("\r")
wait_for("Which SearXNG", 30)
process.write("\r")  # install one
wait_for("Continue?", 10)
process.write("y")
wait_for("Done.", 600)
time.sleep(1)
print(text())
