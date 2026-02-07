# Start

Start a bead using the standard botbox flow: claim the work, set up a workspace, announce.

## Arguments

- `$AGENT` = agent identity (required)
- `<bead-id>` = bead to start (required)

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. `br update --actor $AGENT <bead-id> --status=in_progress --owner=$AGENT`
3. `bus claims stake --agent $AGENT "bead://$BOTBOX_PROJECT/<bead-id>" -m "<bead-id>"`
4. Create a workspace: `maw ws create --random` — note the workspace name and **absolute path** from the output. Store the workspace name as `$WS` and the absolute path as `$WS_PATH`.
5. **All file edits must use the absolute workspace path** shown in the `maw ws create` output (e.g., `/home/user/project/.workspaces/frost-castle/`). Use absolute paths for Read, Write, and Edit tools. For bash commands, prefix with `cd $WS_PATH &&`. For jj commands, use `maw ws jj $WS <args>`. **Do not `cd` into the workspace and stay there** — this breaks cleanup if the workspace is later destroyed. Run `br` commands from the **project root** to prevent beads database merge conflicts.
6. `bus claims stake --agent $AGENT "workspace://$BOTBOX_PROJECT/$WS" -m "<bead-id>"`
7. Announce: `bus send --agent $AGENT $BOTBOX_PROJECT "Working on <bead-id>: <bead-title>" -L task-claim`

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- `maw` workspaces are used (jj required).
