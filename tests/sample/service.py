"""Opt-in #41 acceptance dependency; binds a temporary loopback port."""

import http.server
import json
import os
from pathlib import Path
import sys
import urllib.request

directory = Path(os.environ["KR_RUN_DIR"])
state = directory / "sample-service.json"
run_id = os.environ["KR_RUN_ID"]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(run_id.encode())

    def log_message(self, *args):
        pass


if sys.argv[1] == "start":
    with http.server.HTTPServer(("127.0.0.1", 0), Handler) as server:
        directory.mkdir(parents=True, exist_ok=True)
        state.write_text(json.dumps({
            "port": server.server_port, "pid": os.getpid(), "run_id": run_id,
        }))
        server.serve_forever()
elif sys.argv[1] == "ready":
    data = json.loads(state.read_text())
    assert data["run_id"] == run_id
    with urllib.request.urlopen(
        f"http://127.0.0.1:{data['port']}/health", timeout=1
    ) as response:
        assert response.read().decode() == run_id
elif sys.argv[1] == "cleanup":
    data = json.loads(state.read_text())
    try:
        os.kill(data["pid"], 0)
    except ProcessLookupError:
        pass
    else:
        raise RuntimeError("service process survived cleanup")
    import socket
    with socket.socket() as connection:
        connection.settimeout(1)
        assert connection.connect_ex(("127.0.0.1", data["port"])) != 0
    state.unlink()
    (directory / "sample-service-cleanup.json").write_text(
        json.dumps({"pid_gone": True, "port_closed": True, "run_id": run_id})
    )
    print("service process reaped; port closed; state removed")
