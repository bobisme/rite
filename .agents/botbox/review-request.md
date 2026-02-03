# Review Request

Request a review using crit and announce it in the project channel.

## Arguments

- `$AGENT` = agent identity (required)
- `<review-id>` = review to request (required)
- `<reviewer>` = reviewer agent name (optional)
  - Specialist reviewers follow the pattern: `<project>-<role>`
  - Example: `botbus-security`
  - Common role: `security`

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. If requesting a **specialist reviewer** (e.g., security):
   - Assign them: `crit reviews request <review-id> --reviewers <reviewer> --agent $AGENT`
   - Announce with @mention: `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id>, @<reviewer>" -L review-request`
   - The @mention will trigger auto-spawn hooks to start the specialist reviewer
3. If requesting a **general code review** (no specific specialist):
   - Spawn a subagent to perform the code review
   - Announce: `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id>, spawned subagent for review" -L review-request`

The reviewer-loop finds open reviews via `crit reviews list` and processes them automatically.

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
