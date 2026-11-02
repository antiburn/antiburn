"""Synthetic pinned-shape service for the real Windows-to-WSL bridge test."""

import base64
import json
import os
import secrets
import shlex
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


def install_cli_trap(home, cutoff):
    query = (
        "WITH RECURSIVE ancestors(id, parent_id) AS ("
        f"SELECT id, parent_id FROM session WHERE time_updated >= {cutoff * 1000} "
        "UNION "
        "SELECT parent.id, parent.parent_id FROM session parent "
        "JOIN ancestors child ON child.parent_id = parent.id"
        "), roots(id) AS (SELECT id FROM ancestors WHERE parent_id IS NULL), "
        "cluster(id) AS (SELECT id FROM roots UNION ALL SELECT child.id FROM session child "
        "JOIN cluster parent ON child.parent_id = parent.id) "
        "SELECT session.id, session.parent_id, session.directory, session.title, "
        "session.time_created, session.time_updated FROM session JOIN cluster USING (id)"
    )
    args = ["--pure", "db", query, "--format", "json"]
    checks = (
        ['[ "$#" -eq 5 ]']
        + [
            f'[ "${index}" = {shlex.quote(value)} ]'
            for index, value in enumerate(args, 1)
        ]
        + [
            f'[ "${{{name}-}}" = true ]'
            for name in (
                "OPENCODE_DISABLE_AUTOUPDATE",
                "OPENCODE_DISABLE_MODELS_FETCH",
                "OPENCODE_DISABLE_DEFAULT_PLUGINS",
                "OPENCODE_SKIP_MIGRATIONS",
            )
        ]
    )
    trap = home / ".opencode/bin/opencode"
    trap.parent.mkdir(parents=True, exist_ok=True)
    trap.write_text(
        "#!/bin/sh\nset -eu\n"
        "deny() {\n"
        '  printf "%s\\n" "$@" >> "$HOME/unexpected-opencode-command"\n'
        "  exit 97\n"
        "}\n"
        + "".join(f'{check} || deny "$@"\n' for check in checks)
        + f"printf '%s\\n' {shlex.quote(json.dumps(args))} >> \"$HOME/opencode-commands.jsonl\"\n"
        + "printf '[]\\n'\n"
    )
    trap.chmod(0o700)
    return args


def serve(home):
    if Path.home() != home:
        raise RuntimeError("The service must use the isolated test HOME")
    state = home / ".local/state/opencode"
    state.mkdir(parents=True, exist_ok=True)
    registration = state / "service.json"
    if registration.exists():
        raise RuntimeError("The test must not replace a service registration")
    password = secrets.token_hex(32)
    authorization = (
        "Basic " + base64.b64encode(("opencode:" + password).encode()).decode()
    )
    stamp = int(time.time()) * 1000
    info = {
        "id": "ses_wsl_acceptance",
        "projectID": "synthetic-project",
        "location": {"directory": "/work/synthetic-project"},
        "title": "Synthetic WSL transfer",
        "time": {"created": stamp, "updated": stamp},
        "model": {"id": "synthetic-model", "providerID": "synthetic-provider"},
        "tokens": {
            "input": 0,
            "output": 0,
            "reasoning": 0,
            "cache": {"read": 0, "write": 0},
        },
        "cost": 0,
    }
    transfer = {
        "data": {
            "info": info,
            "messages": [
                {
                    "id": "msg_user",
                    "type": "user",
                    "text": "synthetic pipe request: caf\u00e9 \u96ea\nsecond line",
                    "time": {"created": stamp},
                },
                {
                    "id": "msg_assistant",
                    "type": "assistant",
                    "agent": "build",
                    "model": info["model"],
                    "time": {"created": stamp, "completed": stamp + 1},
                    "content": [
                        {
                            "id": "call_read",
                            "type": "tool",
                            "name": "read",
                            "state": {
                                "status": "completed",
                                "input": {"path": "synthetic.txt"},
                                "content": [
                                    {"type": "text", "text": "x" * (256 * 1024 + 17)}
                                ],
                            },
                            "time": {"created": stamp, "completed": stamp + 1},
                        }
                    ],
                },
            ],
        }
    }
    responses = {
        "/api/info": {"pid": os.getpid(), "version": "2.0.32"},
        "/api/session?limit=100&order=desc": {"data": [info], "cursor": {}},
        "/api/experimental/session/ses_wsl_acceptance/export": transfer,
    }
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(2)

        def log_message(self, *_args):
            pass

        def do_GET(self):
            authorized = self.headers.get("Authorization") == authorization
            status = 200 if authorized and self.path in responses else 403
            body = json.dumps(
                responses[self.path] if status == 200 else {}, ensure_ascii=False
            ).encode()
            requests.append(
                {
                    "method": "GET",
                    "path": self.path,
                    "authorized": authorized,
                    "status": status,
                    "bytes": len(body),
                }
            )
            (home / "requests.json").write_text(json.dumps(requests))
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

    install_cli_trap(home, stamp // 1000 - 60)
    with HTTPServer(("127.0.0.1", 0), Handler) as server:
        server.timeout = 0.1
        descriptor = os.open(registration, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as file:
            json.dump(
                {
                    "id": "synthetic-wsl-acceptance",
                    "pid": os.getpid(),
                    "version": "2.0.32",
                    "url": "http://127.0.0.1:" + str(server.server_port),
                    "password": password,
                },
                file,
            )
        ready = home / "ready.json"
        ready.with_suffix(".tmp").write_text(json.dumps({"updated": stamp // 1000}))
        ready.with_suffix(".tmp").replace(ready)
        try:

            def timeout(_signal, _frame):
                raise SystemExit("The isolated service reached its deadline")

            signal.signal(signal.SIGALRM, timeout)
            signal.alarm(120)
            deadline = time.monotonic() + 120
            while not (home / "stop").exists() and time.monotonic() < deadline:
                server.handle_request()
        finally:
            signal.alarm(0)
            registration.unlink(missing_ok=True)


if __name__ == "__main__":
    serve(Path(sys.argv[1]))
