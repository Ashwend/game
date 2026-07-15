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
  ashwend-control.py <socket> select-actionbar-item <item_id>  # select the actionbar slot holding item_id (raises its placement ghost)
  ashwend-control.py <socket> place-deployable <item_id> [distance] [height]  # drop a carried structure in front, facing you; height = platform top for on-floor placement
  ashwend-control.py <socket> place-building <piece> [distance] [height]  # foundation|wall|window_wall|doorway|ceiling|stairs, server snaps; height raises a free foundation
  ashwend-control.py <socket> place-door <code> [flip]   # hang a carried door in the nearest doorway
  ashwend-control.py <socket> door-interact              # E-press the nearest door
  ashwend-control.py <socket> open-storage-box          # open the nearest storage box's transfer UI
  ashwend-control.py <socket> close-container            # close the open loot-bag/sleeper/storage-box panel
  ashwend-control.py <socket> upgrade-building [piece]   # hammer-upgrade the nearest building block (optionally one piece kind)
  ashwend-control.py <socket> demolish-building [piece]  # hammer-demolish the nearest building block (cascade follows)
  ashwend-control.py <socket> door-enter-code <code>     # enter a code at the nearest door
  ashwend-control.py <socket> set-look <yaw> <pitch>     # absolute radians; pitch clamped like mouse look
  ashwend-control.py <socket> set-screen <name>          # main_menu|worlds|multiplayer|options|in_game
  ashwend-control.py <socket> set-inventory-open <true|false>
  ashwend-control.py <socket> set-crafting-open <true|false>  # open the unified panel on the Crafting tab (stands in for the C hotkey)
  ashwend-control.py <socket> equip-item <item_id>       # equip a wearable from the bag into its paperdoll slot (stands in for shift-click)
  ashwend-control.py <socket> set-world-map-open <true|false>  # open/close the map overlay (bypasses focus gate); opening pulls terrain + markers
  ashwend-control.py <socket> add-world-map-marker <x> <z>     # drop a map marker at world (x, z), as if right-clicking the map
  ashwend-control.py <socket> set-world-map-view <zoom> <cx> <cz>  # set map pan/zoom (zoom 1 = whole world; centre at world cx,cz)
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
        "select-actionbar-item": lambda: {
            "command": "select_actionbar_item",
            "item_id": rest[0],
        },
        "place-deployable": lambda: {
            "command": "place_deployable",
            "item_id": rest[0],
            **({"distance": float(rest[1])} if len(rest) > 1 else {}),
            **({"height": float(rest[2])} if len(rest) > 2 else {}),
        },
        "place-building": lambda: {
            "command": "place_building",
            "piece": rest[0],
            **({"distance": float(rest[1])} if len(rest) > 1 else {}),
            **({"height": float(rest[2])} if len(rest) > 2 else {}),
        },
        "place-door": lambda: {
            "command": "place_door",
            "code": rest[0],
            **(
                {"flip": rest[1].lower() in ("1", "true", "yes", "on")}
                if len(rest) > 1
                else {}
            ),
        },
        "door-interact": lambda: {"command": "door_interact"},
        "door-pickup": lambda: {"command": "door_pick_up"},
        "open-storage-box": lambda: {"command": "open_storage_box"},
        "close-container": lambda: {"command": "close_container"},
        "upgrade-building": lambda: {
            "command": "upgrade_building",
            **({"piece": rest[0]} if rest else {}),
        },
        "demolish-building": lambda: {
            "command": "demolish_building",
            **({"piece": rest[0]} if rest else {}),
        },
        "door-enter-code": lambda: {"command": "door_enter_code", "code": rest[0]},
        "set-look": lambda: {
            "command": "set_look",
            "yaw": float(rest[0]),
            "pitch": float(rest[1]),
        },
        "warp": lambda: {"command": "warp", "x": float(rest[0]), "z": float(rest[1])},
        "walk": lambda: {
            "command": "walk",
            "seconds": float(rest[0]),
            **({"run": rest[1].lower() in ("1", "true", "yes", "on")} if len(rest) > 1 else {}),
        },
        "swing": lambda: {"command": "swing"},
        # Force the ranged bow / crossbow / melee / bandage viewmodel pose for
        # headless capture (dev-only). Pass key=value tokens: draw=<0..1>,
        # reload=<0..1>, recoil=<0..1>, swing=<0..1>, use_charge=<0..1>. No tokens
        # clears the override back to live input.
        "ranged-pose-debug": lambda: {
            "command": "ranged_pose_debug",
            **{
                tok.split("=", 1)[0]: float(tok.split("=", 1)[1])
                for tok in rest
                if "=" in tok
            },
        },
        "set-screen": lambda: {"command": "set_screen", "screen": rest[0]},
        "set-inventory-open": lambda: {
            "command": "set_inventory_open",
            "open": rest[0].lower() in ("1", "true", "yes", "on"),
        },
        "equip-item": lambda: {"command": "equip_item", "item_id": rest[0]},
        "set-crafting-open": lambda: {
            "command": "set_crafting_open",
            "open": rest[0].lower() in ("1", "true", "yes", "on"),
        },
        "set-world-map-open": lambda: {
            "command": "set_world_map_open",
            "open": rest[0].lower() in ("1", "true", "yes", "on"),
        },
        "add-world-map-marker": lambda: {
            "command": "add_world_map_marker",
            "x": float(rest[0]),
            "z": float(rest[1]),
        },
        "set-world-map-view": lambda: {
            "command": "set_world_map_view",
            "zoom": float(rest[0]),
            "center_x": float(rest[1]),
            "center_z": float(rest[2]),
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
