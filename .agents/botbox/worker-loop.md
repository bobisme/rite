# Worker Loop

Full worker lifecycle — triage, start, work, finish, repeat. This is the "colleague" agent: it shows up, finds work, does it, cleans up, and repeats until there is nothing left.

## Identity

If spawned by `agent-loop.sh`, your identity is provided as `$AGENT` (a random name like `storm-raven`). Otherwise, adopt `<project>-dev` as your name (e.g., `botbox-dev`). Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it. It will generate a name if one isn't set.

Your project channel is `$BOTBOX_PROJECT`. All bus commands must include `--agent $AGENT`. All announcements go to `$BOTBOX_PROJECT` with appropriate labels (e.g., `-L task-claim`, `-L review-request`).

**Important:** Run all `br` commands (`br update`, `br close`, `br comments`, `br sync`) from the **project root**, not from inside `.workspaces/$WS/`. This prevents merge conflicts in the beads database. Use absolute paths for file operations in the workspace — **do not `cd` into the workspace and stay there**, as this breaks cleanup when the workspace is destroyed.

## Loop

### 0. Resume check — handle in-progress reviews

Before triaging new work, check if you have an in-progress bead from a previous iteration:

- `bus claims --agent $AGENT` — look for `bead://` claims
- If you hold a bead claim:
  - Find the review ID: `br comments <bead-id>` — look for the "Review requested: <review-id>" comment
  - Check review status: `crit review <review-id>`
  - **LGTM (approved)**: Follow [merge-check](merge-check.md), then go to step 6 (Finish)
  - **Blocked (changes requested)**: Follow [review-response](review-response.md) to fix issues and re-request review. Then STOP this iteration.
  - **Pending (no new activity)**: STOP this iteration. The reviewer has not responded yet.
- If no active claims: proceed to step 1 (Triage).

### 1. Triage — find and groom work, then pick one small task (always run this, even if you already know what to work on)

- Check inbox: `bus inbox --agent $AGENT --channels $BOTBOX_PROJECT --mark-read`
- For messages that request work, create beads: `br create --actor $AGENT --owner $AGENT --title="..." --description="..." --type=task --priority=2`
- For questions or status checks, reply directly: `bus send --agent $AGENT <channel> "<reply>" -L triage-reply`
- Check ready beads: `br ready`
- If no ready beads and no new beads from inbox, stop with message "No work available."
- **Check blocked beads** for resolved blockers: if a bead was blocked pending information or an upstream fix that has since landed, unblock it with `br update --actor $AGENT <id> --status=open` and a comment noting why.
- **Groom each ready bead** (`br show <id>`): ensure it has a clear title, description with acceptance criteria and testing strategy, appropriate priority, and labels. Fix anything missing and comment what you changed.
- Pick one task: `bv --robot-next` — parse the JSON to get the bead ID.
- If the task is large (epic or multi-step), decompose it:
  1. **Groom the parent** first — add labels, refine acceptance criteria, note any discrepancies between the description and the actual project state (e.g., bead says "use SQLite" but no DB crate in dependencies). Comment your findings on the parent bead.
  2. **Create child beads** with `br create --actor $AGENT --owner $AGENT` — each one a resumable unit of work. Titles in imperative form. Descriptions must include acceptance criteria (what "done" looks like).
  3. **Set priorities** that reflect execution order — foundation subtasks get higher priority than downstream features, tests get lowest.
  4. **Wire dependencies** with `br dep add --actor $AGENT <child> <parent>`. Look for parallelism — tasks that share a prerequisite but don't depend on each other should not be chained linearly.
  5. **Comment your decomposition plan** on the parent bead: what you created, why, and any decisions you made (e.g., "Using in-memory storage instead of SQLite because no DB crate available").
  6. **Verify** with `br dep tree <parent>` — the graph should have at least one point where multiple tasks are unblocked simultaneously.
  7. Run `bv --robot-next` again. Repeat until you have exactly one small, atomic task.
- If the bead is claimed by another agent (`bus claims check --agent $AGENT "bead://$BOTBOX_PROJECT/<id>"`), skip it and pick the next recommendation. If all are claimed, stop with "No work available."

### 2. Start — claim and set up

- `br update --actor $AGENT <bead-id> --status=in_progress`
- `bus claims stake --agent $AGENT "bead://$BOTBOX_PROJECT/<bead-id>" -m "<bead-id>"`
- `maw ws create --random` — note the workspace name (e.g., `frost-castle`) and the **absolute path** from the output. Store as `$WS` (name) and `$WS_PATH` (absolute path).
- **All file operations must use the absolute workspace path** from `maw ws create` output. Use absolute paths for Read, Write, and Edit. For bash: `cd $WS_PATH && <command>`. For jj: `maw ws jj $WS <args>`. **Do not `cd` into the workspace and stay there** — the workspace will be destroyed during finish, breaking your shell session.
- `bus claims stake --agent $AGENT "workspace://$BOTBOX_PROJECT/$WS" -m "<bead-id>"`
- `bus send --agent $AGENT $BOTBOX_PROJECT "Working on <bead-id>: <bead-title>" -L task-claim`

### 3. Work — implement the task

- Read the bead details: `br show <bead-id>`
- Do the work using the tools available in the workspace.
- **You must add at least one progress comment** during work: `br comments add --actor $AGENT --author $AGENT <bead-id> "Progress: ..."`
  - Post when you've made meaningful progress or hit a milestone
  - This is required before you can close the bead — do not skip it
  - Essential for visibility and debugging if something goes wrong

### 4. Stuck check — recognize when you are stuck

You are stuck if: you attempted the same approach twice without progress, you cannot find needed information or files, or a tool command fails repeatedly.

If stuck:
- Add a detailed comment with what you tried and where you got blocked: `br comments add --actor $AGENT --author $AGENT <bead-id> "Blocked: ..."`
- Post in the project channel: `bus send --agent $AGENT $BOTBOX_PROJECT "Stuck on <bead-id>: <summary>" -L task-blocked`
- If a tool behaved unexpectedly (e.g., command succeeded but had no effect), also report it: `bus send --agent $AGENT $BOTBOX_PROJECT "Tool issue: <tool> <what happened>" -L tool-issue`
- `br update --actor $AGENT <bead-id> --status=blocked`
- Release the bead claim: `bus claims release --agent $AGENT "bead://$BOTBOX_PROJECT/<bead-id>"`
- Move on to triage again (go to step 1).

### 5. Review request — submit work for review

After completing the implementation:

- Describe the change: `maw ws jj $WS describe -m "<bead-id>: <summary>"`
- Create a crit review with bead context: `crit reviews create --agent $AGENT --title "<bead-title>" --description "For <bead-id>: <summary of changes, what was done, why>"`
  - Always include the bead ID in the description so reviewers have context
  - Explain what changed and why, not just a summary
- Add a comment to the bead: `br comments add --actor $AGENT --author $AGENT <bead-id> "Review requested: <review-id>, workspace: $WS ($WS_PATH)"`
- **If requesting a specialist reviewer** (e.g., security):
  - Assign them: `crit reviews request <review-id> --reviewers <reviewer> --agent $AGENT`
  - Announce with @mention: `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id> for <bead-id>, @<reviewer>" -L review-request`
  - The @mention triggers auto-spawn hooks
- **If requesting a general code review**:
  - Spawn a subagent to perform the review
  - Announce: `bus send --agent $AGENT $BOTBOX_PROJECT "Review requested: <review-id> for <bead-id>, spawned subagent for review" -L review-request`
- **STOP this iteration.** Do NOT close the bead, merge the workspace, or release claims. The reviewer will process the review, and you will resume in the next iteration via step 0.

See [review-request](review-request.md) for full details.

### 6. Finish — mandatory teardown (never skip)

If a review was conducted:
- Verify approval: `crit review <review-id>` — confirm LGTM, no blocks
- Mark review as merged: `crit reviews merge <review-id> --agent $AGENT`

Then proceed with teardown:
- `br comments add --actor $AGENT --author $AGENT <bead-id> "Completed by $AGENT"`
- `br close --actor $AGENT <bead-id> --reason="Completed" --suggest-next`
- `maw ws merge $WS --destroy` (if merge conflict, preserve workspace and announce)
- `bus claims release --agent $AGENT --all`
- `br sync --flush-only`
- `bus send --agent $AGENT $BOTBOX_PROJECT "Completed <bead-id>: <bead-title>" -L task-done`

### 7. Repeat

Go back to step 0. The loop ends when triage finds no work and no reviews are pending.

## Key Rules

- **Exactly one small task at a time.** Never work on multiple beads concurrently.
- **Always finish or release before picking new work.** Context must be clear.
- **If claim is denied, back off and pick something else.** Never force or wait.
- **All bus commands use `--agent $AGENT`.**
