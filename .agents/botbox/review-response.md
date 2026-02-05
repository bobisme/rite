# Review Response

Handle reviewer feedback on a blocked or commented review. For each thread, decide whether to fix, address, or defer.

Your identity is `$AGENT`. All crit and bus commands must include `--agent $AGENT`. Run `bus whoami --agent $AGENT` first if you need to confirm the identity.

## Arguments

- `$AGENT` = agent identity (required)
- `<review-id>` = review to respond to (required)

## When to Use

Run this when:
- `crit inbox --agent $AGENT --all-workspaces` shows threads with new comments on your review
- `bus inbox` contains a `review-done` message indicating your review was blocked
- You previously requested review and are checking back for feedback

**Note:** All crit commands below require `--path $WS_PATH` because the review exists in your workspace, not the repo root.

## Steps

1. Read the review and all threads: `crit review <review-id> --path $WS_PATH`
2. For each thread with reviewer feedback, categorize by severity and decide:

   **Fix** (CRITICAL or HIGH severity — must resolve before merge):
   - Make the code change in the workspace
   - Reply on the thread: `crit reply <thread-id> --agent $AGENT --path $WS_PATH "Fixed: <description>"`

   **Address** (reviewer concern is valid but current approach is correct):
   - Reply explaining why: `crit reply <thread-id> --agent $AGENT --path $WS_PATH "Won't fix: <rationale>"`
   - Be specific — reference docs, compiler output, or design intent

   **Defer** (good idea, but out of scope for this change):
   - Create a tracking bead: `br create --actor $AGENT --owner $AGENT "<title>" --label deferred`
   - Reply: `crit reply <thread-id> --agent $AGENT --path $WS_PATH "Deferred to <bead-id> for follow-up"`

3. After handling all threads:
   a. Verify fixes compile: `cargo check` (or equivalent for the project)
   b. Describe the change: `maw ws jj $WS describe -m "fix: address review feedback on <review-id>"`
   c. Re-request review: `crit reviews request <review-id> --agent $AGENT --path $WS_PATH --reviewers <reviewer>`
   d. Announce (include workspace path so the reviewer can find the fixed code):
      `bus send --agent $AGENT $BOTBOX_PROJECT "Review feedback addressed: <review-id>, fixes in workspace $WS ($WS_PATH)" -L review-response`

## After LGTM

When the reviewer approves:

1. Verify approval: `crit review <review-id> --path $WS_PATH` — confirm LGTM vote, no blocks
2. Mark review as merged: `crit reviews mark-merged <review-id> --agent $AGENT --path $WS_PATH`
3. Continue with [finish](finish.md) to close the bead and merge the workspace

The actual code merge is handled by `maw ws merge` in the finish step — do not run `jj squash` manually.

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- You are the author of the review (the agent that created it or requested it).
- The workspace is still active — fixes are made in the workspace, not the main branch.
