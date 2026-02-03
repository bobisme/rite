# Triage

Find exactly one actionable bead, or determine there is no work available. Groom beads along the way to keep the backlog healthy.

## Arguments

- `$AGENT` = agent identity (required)

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. Check inbox for new messages:
   - `bus inbox --agent $AGENT --channels $BOTBOX_PROJECT --mark-read`
   - For each message that requests work (task request, bug report, feature ask), create a bead: `br create --actor $AGENT --owner $AGENT --title="..." --description="..." --type=task --priority=2`
   - For messages with `-L feedback` (reports from other agents):
     - Review the mentioned bead IDs with `br show <bead-id>`
     - Triage the beads (accept, adjust priority, close if duplicate/out-of-scope)
     - Respond on botbus: `bus send --agent $AGENT <channel> "Triaged N beads: <summary> @<reporter-agent>" -L mesh -L triage-reply`
   - For messages that are questions or status checks, reply inline: `bus send --agent $AGENT <channel> "<response>" -L mesh -L triage-reply`
3. Check for ready beads: `br ready`
   - If no ready beads exist and no inbox messages created new beads, output `NO_WORK_AVAILABLE` and stop.
4. **Check blocked beads** for resolved blockers. If a bead was blocked pending information, an upstream fix, or a tool issue that has since been resolved, unblock it: `br update --actor $AGENT <bead-id> --status=open` with a comment explaining why it's unblocked.
5. **Groom the ready beads.** For each bead from `br ready`, run `br show <bead-id>` and fix anything missing:
   - **Title**: Should be clear and actionable (imperative form, e.g., "Add /health endpoint"). If vague, update: `br update --actor $AGENT <bead-id> --title="..."`
   - **Description**: Should explain what and why. If missing or vague, add context: `br update --actor $AGENT <bead-id> --description="..."`
   - **Priority**: Should reflect relative importance. Adjust if wrong: `br update --actor $AGENT <bead-id> --priority=<1-4>`
   - **Labels**: Add labels if the bead fits a category (see label conventions). Apply with `br label add --actor $AGENT -l <label> <bead-id>` (creates label automatically if it doesn't exist).
   - **Acceptance criteria**: Description should include what "done" looks like. If missing, append criteria to the description.
   - **Testing strategy**: Description should mention how to verify the work (e.g., "run tests", "manual check", "curl endpoint"). If missing, append a brief testing note.
   - Add a comment noting what you groomed: `br comments add --actor $AGENT --author $AGENT <bead-id> "Groomed by $AGENT: <what changed>"`
6. Use bv to pick exactly one task: `bv --robot-next`
   - Parse the JSON output to get the recommended bead ID.
7. Check the bead size: `br show <bead-id>`
   - If the bead is large (epic, or description suggests multiple distinct changes), break it down:
     - Create smaller child beads with `br create --actor $AGENT --owner $AGENT` and `br dep add --actor $AGENT <child> <parent>`.
     - Then run `bv --robot-next` again to pick one of the children.
   - Repeat until you have exactly one small, atomic task.
8. Verify the bead is not claimed by another agent: `bus claims check --agent $AGENT "bead://$BOTBOX_PROJECT/<bead-id>"`
   - If claimed by someone else, back off and run `bv --robot-next` again excluding that bead.
   - If all candidates are claimed, output `NO_WORK_AVAILABLE` and stop.
9. Output the single bead ID as the result.

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- `bv` is available and the beads database is initialized.
- The agent will use the [start](start.md) workflow next to claim and begin work.
