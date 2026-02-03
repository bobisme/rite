# Review Request

Request a review using crit and announce it in the project channel.

## Arguments

- `$AGENT` = agent identity (required)
- `<review-id>` = review to request (required)
- `<reviewer>` = reviewer role or agent name (optional)

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. If a specific reviewer is known: `crit reviews request <review-id> --reviewers <reviewer> --agent $AGENT`.
3. `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id>" -L mesh -L review-request`

The bus announcement with `-L review-request` is what triggers reviewer agents to pick up the review. The `crit reviews request` step is optional — it assigns a specific reviewer, but the reviewer-loop finds open reviews via `crit reviews list` regardless.

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
