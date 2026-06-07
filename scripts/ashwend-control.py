#!/usr/bin/env python3
"""Drive the Ashwend dev client control socket.

The client opens this Unix socket only when GAME_CONTROL_SOCKET is set (see
src/app/systems/control_socket.rs). It speaks one line-delimited JSON request
per connection and replies with {"ok": bool, "message": str}. This is a thin,
stdlib-only driver so an agent can launch the game, act, and assert on state
without reading pixels.

Usage:
  ashwend-control.py <socket> dump-state
  ashwend-control.py <socket> screenshot <png-path>
  ashwend-control.py <socket> send-command <text>        # slash command, no leading '/'
  ashwend-control.py <socket> select-actionbar-slot <n>  # 0-based; puts that slot's item in hand
  ashwend-control.py <socket> set-screen <name>          # main_menu|worlds|multiplayer|options|in_game
  ashwend-control.py <socket> set-inventory-open <true|false>
  ashwend-control.py <socket> wait-in-world [timeout_s]   # poll dump-state until in_world

Exit code is non-zero when the request fails (ok == false) or times out.
"""

import json
import socket
import sys
import time


def send(sock_path, request, timeout=20.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(sock_path)
    s.sendall(json.dumps(request).encode())
    s.shutdown(socket.SHUT_WR)
    data = b""
    while True:
        chunk = s.recv(4096)
        if not chunk:
            break
        data += chunk
    s.close()
    return json.loads(data.decode().strip())


def wait_in_world(sock_path, timeout_s):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        resp = send(sock_path, {"command": "dump_state"})
        if resp.get("ok"):
            state = json.loads(resp["message"])
            if state.get("in_world"):
                return state
        time.sleep(0.25)
    raise TimeoutError(f"world not ready within {timeout_s}s")


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    sock_path, action = argv[1], argv[2]
    rest = argv[3:]

    if action == "wait-in-world":
        timeout_s = float(rest[0]) if rest else 30.0
        state = wait_in_world(sock_path, timeout_s)
        print(json.dumps(state, indent=2))
        return 0

    request = {
        "dump-state": lambda: {"command": "dump_state"},
        "screenshot": lambda: {"command": "screenshot", "path": rest[0]},
        "send-command": lambda: {"command": "send_command", "text": rest[0]},
        "select-actionbar-slot": lambda: {
            "command": "select_actionbar_slot",
            "slot": int(rest[0]),
        },
        "set-screen": lambda: {"command": "set_screen", "screen": rest[0]},
        "set-inventory-open": lambda: {
            "command": "set_inventory_open",
            "open": rest[0].lower() in ("1", "true", "yes", "on"),
        },
    }.get(action)
    if request is None:
        print(f"unknown action: {action}\n{__doc__}")
        return 2

    resp = send(sock_path, request())
    # dump_state returns a JSON blob in message; pretty-print it.
    if action == "dump-state" and resp.get("ok"):
        print(json.dumps(json.loads(resp["message"]), indent=2))
    else:
        print(resp.get("message", resp))
    return 0 if resp.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
