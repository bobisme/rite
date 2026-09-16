# Evidence for notes/agent-sessions.md (2026-09-15)

**Reconstruction notice.** The scripts below were written in a session
scratchpad that was lost when the host rebooted before they were copied here.
They are reproduced from the session record that wrote them, byte-for-byte as
far as that record shows. The bridge logs, channel log, and hook dumps were
lost and are not reproduced. The bus messages themselves are durable: every
request and reply is on `#rite` under label `probe`.

```bash
rite history rite -L probe -n 40 --format json
rite history --thread 01M2KFP4Q4HZSJ82SDGXH91FVE   # Claude -> Codex, busy
rite history --thread 01M2KGC42B7SZV0BW6WEBRYK1J   # Codex A -> B
rite history --thread 01M2KGEN9SS6Z9RX9R1TEET3A1   # Codex B -> A
```

| File | Role |
|---|---|
| `bridge.sh` | Codex push adapter, single identity: mention stream into `codex queue` |
| `bridge2.sh` | Same, identity as first argument, used for the two-Codex test |
| `rite_channel.py` | Claude channel adapter: stdio MCP server, no SDK |
| `mcp.json` | `.mcp.json` entry used for the Claude probe |
| `probe-claude.CLAUDE.md` | Instructions for the Claude probe |
| `probe-codex.AGENTS.md` | Instructions for the first Codex probe (later replaced by prompts) |
| `hookdump.sh` | Codex hook that dumped stdin, pid, parent, and environment |
| `hooks.json` | Project-level `.codex/hooks.json` used for the hook test |

Timing boundaries used in the note: "queue call" is the bridge's `ts`, taken
before `codex queue` runs; "notification write" is the Python adapter's log
line after writing to the transport. Neither is receipt.
