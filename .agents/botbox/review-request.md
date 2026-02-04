# Review Request

Request a review using crit and announce it in the project channel.

## Arguments

- `$AGENT` = agent identity (required)
- `<review-id>` = review to request (required)
- `<reviewer>` = reviewer agent name (optional)
  - Specialist reviewers follow the pattern: `<project>-<role>`
  - Example: `myproject-security`
  - Common role: `security`

## How Reviewer Spawning Works

**Important**: There are TWO steps to request a specialist reviewer:

1. **crit assignment** — Records who should review (metadata only, does NOT spawn)
2. **@mention in bus message** — Triggers the auto-spawn hook (THIS spawns the reviewer)

The botbus hook system watches for @mentions. When you send a message containing `@myproject-security`, the hook spawns the security reviewer agent. The crit `--reviewers` flag alone does nothing to spawn — it only records the assignment.

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.

2. If requesting a **specialist reviewer** (e.g., security):
   ```bash
   # Step 1: Assign reviewer in crit (records who should review)
   crit reviews request <review-id> --reviewers $BOTBOX_PROJECT-security --agent $AGENT

   # Step 2: Announce with @mention (TRIGGERS THE SPAWN)
   bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id> @$BOTBOX_PROJECT-security" -L review-request
   ```

   The reviewer name MUST match the project pattern: `<project>-<role>` (e.g., `myproject-security`, `botbus-security`). Do NOT use generic names like `security-reviewer` — those won't match any hooks.

3. If requesting a **general code review** (no specific specialist):
   - Spawn a subagent to perform the code review
   - Announce: `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id>, spawned subagent for review" -L review-request`

The reviewer-loop finds open reviews via `crit reviews list` and processes them automatically.

## Common Mistakes

- Using `--reviewer security` or `--reviewers security-reviewer` — these generic names don't match any hooks
- Forgetting the @mention in the bus message — without it, no reviewer spawns
- Using the wrong project prefix — reviewer must be `<project>-<role>` where `<project>` matches the channel

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
