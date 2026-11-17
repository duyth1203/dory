#!/usr/bin/env bash
# Sends MCP requests to a stdio server and prints each response on one line, then closes stdin.
# Usage: scripts/mcp-probe.sh --legacy|--modern <server binary> <method> [json params] ...
# --legacy runs `initialize` first (protocol 2025-11-25). --modern sends `server/discover` and puts
# the 2026-07-28 request metadata on every request. Params default to {}.
set -euo pipefail
exec python3 -I -c '
import json, subprocess, sys

mode, binary, rest = sys.argv[1], sys.argv[2], sys.argv[3:]
meta = {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientCapabilities": {},
    "io.modelcontextprotocol/clientInfo": {"name": "mcp-probe", "version": "1"},
}
calls = []
if mode == "--legacy":
    calls.append(("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                 "clientInfo": {"name": "mcp-probe", "version": "1"}}))
elif mode == "--modern":
    calls.append(("server/discover", {}))
else:
    sys.exit("first argument is --legacy or --modern")
i = 0
while i < len(rest):
    params = {}
    if i + 1 < len(rest) and rest[i + 1].startswith("{"):
        params = json.loads(rest[i + 1])
        calls.append((rest[i], params))
        i += 2
    else:
        calls.append((rest[i], params))
        i += 1

server = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
for n, (method, params) in enumerate(calls, 1):
    if mode == "--modern":
        params = dict(params, _meta=meta)
    server.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n")
    server.stdin.flush()
    if n == 1 and mode == "--legacy":
        print(server.stdout.readline().rstrip())
        server.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        server.stdin.flush()
        continue
    while True:
        line = server.stdout.readline()
        if not line:
            sys.exit("the server closed stdout")
        if json.loads(line).get("id") == n:
            print(line.rstrip())
            break
server.stdin.close()
sys.exit(server.wait())
' "$@"
