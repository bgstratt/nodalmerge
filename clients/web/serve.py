"""Demo dev server that serves ./web with aggressive no-cache headers.

The default `python -m http.server` sends ETags without `Cache-Control`,
which lets Chrome keep the old `nodalmerge_bridge_bg.wasm` module alive
across hard-reloads (WASM modules can outlive a Ctrl+Shift+R in some
cases). That caused phantom "lamport ceiling exceeded" floods after
rebuilding the bridge. Using this wrapper guarantees the browser always
fetches the freshly built WASM and JS glue.

Run from repo root:

    python web/serve.py            # serves on http://localhost:8080
    python web/serve.py 9000       # serves on http://localhost:9000
"""

from __future__ import annotations

import json
import os
import sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


class NoCacheHandler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "application/javascript",
        ".mjs": "application/javascript",
        ".ts": "application/typescript",
    }

    def end_headers(self) -> None:  # noqa: D401 - stdlib override
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def do_GET(self) -> None:  # noqa: D401 - stdlib override
        if self.path.startswith("/__nodalmerge_config.js") or self.path.startswith("/__activesync_config.js"):
            env_server = (
                os.environ.get("NODALMERGE_SERVER_URL")
                or os.environ.get("NODALMERGE_DEMO_SERVER_URL")
                or os.environ.get("ACTIVESYNC_SERVER_URL")
                or os.environ.get("ACTIVESYNC_DEMO_SERVER_URL")
                or ""
            ).strip()
            payload = {
                "serverUrl": env_server,
            }
            body = (
                "window.NODALMERGE_CONFIG = "
                + json.dumps(payload, separators=(",", ":"))
                + ";\n"
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        super().do_GET()


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), os.pardir))
    web_dir = os.path.join(repo_root, "web")
    handler = partial(NoCacheHandler, directory=web_dir)
    with ThreadingHTTPServer(("127.0.0.1", port), handler) as httpd:
        print(f"nodalmerge demo: http://localhost:{port} (no-cache) serving {web_dir}")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nbye")


if __name__ == "__main__":
    main()
