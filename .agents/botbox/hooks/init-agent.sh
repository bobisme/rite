#!/bin/bash
# botbox SessionStart hook: display agent identity from .botbox.json

# Check if .botbox.json exists and read agent identity from it
if [ -f ".botbox.json" ]; then
    # Extract values using jq (falls back to bus whoami if jq not available)
    if command -v jq &> /dev/null; then
        DEFAULT_AGENT=$(jq -r '.project.defaultAgent // .project.default_agent // empty' .botbox.json 2>/dev/null)
        CHANNEL=$(jq -r '.project.channel // .project.name // empty' .botbox.json 2>/dev/null)

        if [ -n "$DEFAULT_AGENT" ]; then
            echo "Agent ID for use with botbus/crit/br: $DEFAULT_AGENT"
            if [ -n "$CHANNEL" ]; then
                echo "Project channel: $CHANNEL"
            fi
            exit 0
        fi
    fi
fi

# Fallback to bus whoami
AGENT_ID=$(bus whoami --suggest-project-suffix=dev 2>&1)
echo "Agent ID for use with botbus/crit/br: $AGENT_ID"
