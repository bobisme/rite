#!/bin/bash
# Codex hook: dump stdin JSON plus pid, parent, and selected environment to a file.
out="<dir>/hook-$(date +%s%N).json"
{ echo "{\"env_RITE_AGENT\":\"$RITE_AGENT\",\"env_PROBE\":\"$PROBE\",\"pid\":$$,\"ppid\":$PPID,\"ppid_cmd\":\"$(tr '\0' ' ' < /proc/$PPID/cmdline 2>/dev/null | head -c 120)\",\"stdin\":"; cat; echo "}"; } > "$out"
