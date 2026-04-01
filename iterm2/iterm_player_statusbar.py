#!/usr/bin/env python3

import asyncio
import html
import json
import shlex
from pathlib import Path

import iterm2

SOCKET_PATH = "/tmp/iterm-player.sock"
STATE_PATH = "/tmp/iterm-player.json"
PLAYER_BINARY = "iterm-player"
PLAYER_LAUNCH_COMMAND = f"/bin/zsh -lc {shlex.quote(PLAYER_BINARY)}"
FIRST_STATION = "nts1"
HTTP_HOST = "127.0.0.1"
HTTP_PORT = 18941


def read_state():
    try:
        with open(STATE_PATH, "r", encoding="utf-8") as f:
            state = json.load(f)
    except Exception:
        return {
            "running": False,
            "station_key": None,
            "station_label": None,
            "color": "cyan",
            "pid": None,
        }

    return {
        "running": bool(state.get("running")),
        "station_key": state.get("station_key"),
        "station_label": state.get("station_label"),
        "color": state.get("color", "cyan"),
        "pid": state.get("pid"),
    }


async def send_command(command: str):
    reader, writer = await asyncio.open_unix_connection(SOCKET_PATH)
    writer.write((command + "\n").encode("utf-8"))
    await writer.drain()
    data = await reader.readline()
    writer.close()
    await writer.wait_closed()
    text = data.decode("utf-8", errors="replace").strip()
    return json.loads(text) if text else {}


async def wait_for_socket(timeout: float = 6.0):
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        if Path(SOCKET_PATH).exists():
            try:
                await send_command("status")
                return True
            except Exception:
                pass
        await asyncio.sleep(0.2)
    return False


async def ensure_player_running(connection, command=PLAYER_LAUNCH_COMMAND):
    try:
        await send_command("status")
        return True
    except Exception:
        pass

    window = await iterm2.Window.async_create(connection, command=command)
    if window is None:
        return False

    return await wait_for_socket()


async def ensure_playing(connection, command=PLAYER_LAUNCH_COMMAND):
    state = read_state()
    if state["running"]:
        return True

    ok = await ensure_player_running(connection, command)
    if not ok:
        return False

    await send_command(f"play {FIRST_STATION}")
    return True


async def toggle_playback(connection, command=PLAYER_LAUNCH_COMMAND):
    state = read_state()
    if state["running"]:
        await send_command("stop")
        return

    ok = await ensure_player_running(connection, command)
    if not ok:
        return
    await send_command(f"play {FIRST_STATION}")


async def next_station(connection, command=PLAYER_LAUNCH_COMMAND):
    ok = await ensure_player_running(connection, command)
    if not ok:
        return

    state = read_state()
    if not state["running"]:
        await send_command(f"play {FIRST_STATION}")
    else:
        await send_command("next")


def widget_text():
    state = read_state()
    action = "■" if state["running"] else "▶"
    label = state["station_label"] or "Stopped"
    return f"{action} | ▶▶ | {label}"


def popover_html():
    state = read_state()
    action = "■" if state["running"] else "▶"
    label = html.escape(state["station_label"] or "Stopped")
    color = html.escape(state["color"] or "cyan")
    station_key = html.escape(state["station_key"] or "")
    return f"""<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <style>
      body {{
        margin: 0;
        padding: 16px;
        font: 13px -apple-system, BlinkMacSystemFont, sans-serif;
        background: #151a1e;
        color: #e7eef3;
      }}
      .row {{
        display: flex;
        gap: 8px;
        align-items: center;
        margin-bottom: 12px;
      }}
      button {{
        border: 0;
        border-radius: 8px;
        padding: 8px 12px;
        font: inherit;
        background: #24313a;
        color: #e7eef3;
        cursor: pointer;
      }}
      button.primary {{
        background: #2c6b73;
      }}
      .meta {{
        opacity: 0.85;
      }}
      .label {{
        font-weight: 600;
      }}
      .key {{
        opacity: 0.7;
      }}
      .color {{
        display: inline-block;
        margin-top: 8px;
        opacity: 0.75;
      }}
    </style>
  </head>
  <body>
    <div class="row">
      <button class="primary" onclick="sendAction('/toggle')">{html.escape(action)}</button>
      <button onclick="sendAction('/next')">▶▶</button>
    </div>
    <div class="meta">
      <div class="label">{label}</div>
      <div class="key">{station_key}</div>
      <div class="color">Color: {color}</div>
    </div>
    <script>
      async function sendAction(path) {{
        try {{
          await fetch('http://{HTTP_HOST}:{HTTP_PORT}' + path, {{
            method: 'POST',
            mode: 'cors'
          }});
        }} catch (e) {{
          console.error(e);
        }}
      }}
    </script>
  </body>
</html>"""


def http_response(status, body, content_type="application/json"):
    body_bytes = body.encode("utf-8")
    headers = [
        f"HTTP/1.1 {status}",
        f"Content-Type: {content_type}; charset=utf-8",
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Methods: POST, OPTIONS",
        "Access-Control-Allow-Headers: Content-Type",
        f"Content-Length: {len(body_bytes)}",
        "Connection: close",
        "",
        "",
    ]
    return "\r\n".join(headers).encode("utf-8") + body_bytes


async def handle_http(reader, writer, connection):
    try:
        request_line = await reader.readline()
        if not request_line:
            writer.close()
            await writer.wait_closed()
            return

        try:
            method, path, _ = request_line.decode("utf-8", errors="replace").strip().split(" ", 2)
        except ValueError:
            writer.write(http_response("400 Bad Request", '{"ok":false}'))
            await writer.drain()
            writer.close()
            await writer.wait_closed()
            return

        while True:
            line = await reader.readline()
            if not line or line == b"\r\n":
                break

        if method == "OPTIONS":
            writer.write(http_response("204 No Content", ""))
        elif method == "POST" and path == "/toggle":
            await toggle_playback(connection)
            writer.write(http_response("200 OK", '{"ok":true}'))
        elif method == "POST" and path == "/next":
            await next_station(connection)
            writer.write(http_response("200 OK", '{"ok":true}'))
        else:
            writer.write(http_response("404 Not Found", '{"ok":false}'))

        await writer.drain()
    finally:
        writer.close()
        await writer.wait_closed()


async def main(connection):
    component = iterm2.StatusBarComponent(
        short_description="iTerm Player",
        detailed_description="Play, stop, and switch the current iterm-player station",
        knobs=[],
        exemplar="▶ | ▶▶ | Worldwide FM",
        update_cadence=0.5,
        identifier="com.jeremieberduck.iterm-player.controls",
    )

    server = await asyncio.start_server(
        lambda reader, writer: handle_http(reader, writer, connection),
        HTTP_HOST,
        HTTP_PORT,
    )

    @iterm2.StatusBarRPC
    async def player_coro(knobs):
        del knobs
        text = widget_text()
        return [text, text, text]

    @iterm2.RPC
    async def player_click(session_id):
        await component.async_open_popover(
            session_id,
            popover_html(),
            iterm2.Size(260, 140),
        )

    await component.async_register(connection, player_coro, onclick=player_click)

    async with server:
        await server.serve_forever()


iterm2.run_forever(main)
