#!/usr/bin/env python3
"""Minimal stdio MCP channel server: rite mentions -> notifications/claude/channel."""
import json, os, subprocess, sys, threading, datetime
AGENT = os.environ.get("RITE_AGENT", "probe-claude")
LOG = open(os.environ.get("RITE_CHANNEL_LOG", "/dev/null"), "a", buffering=1)
lock = threading.Lock()
def log(s): LOG.write(f"{datetime.datetime.utcnow().isoformat()}Z {s}\n")
def send(obj):
    with lock:
        sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()
def follower():
    p = subprocess.Popen(["rite", "mentions", "follow", "--agent", AGENT, "--format", "json"],
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    for line in p.stdout:
        try: r = json.loads(line)
        except Exception: continue
        m = r["message"]
        send({"jsonrpc": "2.0", "method": "notifications/claude/channel", "params": {
            "content": m["body"],
            "meta": {"from_agent": m["agent"], "channel_name": r["channel"], "reply_target": r["reply_target"],
                     "route": r["route"], "msg_id": str(m["id"])}}})
        log(f"pushed id={m['id']} from={m['agent']}")
TOOLS = [{"name": "reply", "description": "Reply on the rite bus to an inbound channel event.",
          "inputSchema": {"type": "object", "properties": {
              "target": {"type": "string", "description": "meta.reply_target verbatim"},
              "text": {"type": "string"}, "reply_to": {"type": "string", "description": "meta.msg_id verbatim"}},
              "required": ["target", "text", "reply_to"]}}]
for raw in sys.stdin:
    try: req = json.loads(raw)
    except Exception: continue
    mid, method, params = req.get("id"), req.get("method"), req.get("params") or {}
    log(f"recv {method}")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"experimental": {"claude/channel": {}}, "tools": {}},
            "serverInfo": {"name": "rite-probe", "version": "0.1"},
            "instructions": ("Messages from other rite agents arrive as <channel source=\"rite-probe\" from_agent=... "
                             "reply_target=... msg_id=...> events. Reply with the reply tool, passing target=reply_target "
                             "and reply_to=msg_id verbatim, and put @<from_agent> at the start of text.")}})
    elif method == "notifications/initialized":
        threading.Thread(target=follower, daemon=True).start()
    elif method == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        a = params.get("arguments", {})
        cmd = ["rite", "send", "--agent", AGENT, a["target"], a["text"], "--reply-to", a["reply_to"], "-L", "probe", "--format", "json"]
        r = subprocess.run(cmd, capture_output=True, text=True)
        log(f"reply rc={r.returncode} {r.stdout.strip()[:120]} {r.stderr.strip()[:120]}")
        send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": r.stdout or r.stderr}], "isError": r.returncode != 0}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": f"unknown method {method}"}})
