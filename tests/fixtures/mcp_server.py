#!/usr/bin/env python3
"""A minimal MCP server, enough to exercise the proxy's MCP policy end to end.

httpbin cannot stand in for this. The proxy's MCP handling reads a JSON-RPC
envelope out of the request and, since #457, rewrites the listing that comes
back -- neither of which an echo backend produces. Everything here is
deliberately fixed: the point is to assert what the proxy did to a known
response, not to be a real server.

Stdlib only, so it runs on a stock `python:*-alpine` with nothing installed.

A path containing `sse` gets a `text/event-stream` reply and anything else gets
`application/json`, so both framings the proxy has to filter are reachable --
matched anywhere in the path so it survives a route prefix in front of it.
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PROTOCOL_VERSION = "2026-07-28"

# One tool a route is expected to permit, one it is expected to hide, and one
# that only shows up when nothing is filtered.
TOOLS = [
    {
        "name": "search_docs",
        "description": "Search the documentation",
        "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}},
    },
    {
        "name": "get_weather",
        "description": "Current conditions for a city",
        "inputSchema": {"type": "object", "properties": {"city": {"type": "string"}}},
    },
    {
        "name": "execute_sql",
        "description": "Run a query against the production database",
        "inputSchema": {"type": "object", "properties": {"sql": {"type": "string"}}},
    },
]

RESOURCES = [
    {"uri": "file:///public/readme.txt", "name": "readme"},
    {"uri": "file:///secret/credentials.txt", "name": "credentials"},
]


def result_for(method, params):
    """The result payload for a method, or an error tuple."""
    if method == "initialize":
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "zentinel-test-mcp", "version": "1.0.0"},
        }
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "resources/list":
        return {"resources": RESOURCES}
    if method == "tools/call":
        name = params.get("name", "")
        return {"content": [{"type": "text", "text": f"called {name}"}]}
    return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # noqa: A003 - stdlib hook
        sys.stderr.write("mcp-server: " + fmt % args + "\n")

    def do_GET(self):
        # A liveness probe for compose, not part of MCP.
        self._send(200, b'{"status":"ok"}', "application/json")

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""

        try:
            envelope = json.loads(raw)
        except ValueError:
            self._send(400, b'{"error":"not json"}', "application/json")
            return

        method = envelope.get("method", "")
        result = result_for(method, envelope.get("params") or {})

        if result is None:
            body = {
                "jsonrpc": "2.0",
                "id": envelope.get("id"),
                "error": {"code": -32601, "message": f"unknown method {method}"},
            }
        else:
            body = {"jsonrpc": "2.0", "id": envelope.get("id"), "result": result}

        payload = json.dumps(body).encode()

        if "sse" in self.path:
            # Streamable HTTP: the response arrives inside an SSE event. The
            # stream is closed straight after, which is what a server that has
            # nothing further to say should do.
            framed = b"event: message\ndata: " + payload + b"\n\n"
            self._send(200, framed, "text/event-stream")
        else:
            self._send(200, payload, "application/json")

    def _send(self, status, body, content_type):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("MCP-Protocol-Version", PROTOCOL_VERSION)
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8090
    print(f"mcp-server listening on {port}", file=sys.stderr, flush=True)
    HTTPServer(("0.0.0.0", port), Handler).serve_forever()
