# Report Issue

Report a bug, feature request, or feedback to another project.

## When to use

- You encountered a bug or limitation in a tool (botty, br, crit, etc.)
- You have a feature suggestion for another project
- You need to provide testing feedback or usage notes

## Steps

1. **Identify the project** that owns the tool:
   ```bash
   bus history projects --format text | grep "tools:.*<toolname>"
   ```
   Parse the message to extract:
   - `repo:<path>` — repository path
   - `lead:<agent>` — lead agent to notify
   - `project:<name>` — project/channel name

2. **Navigate to the project repo:**
   ```bash
   cd <repo-path>
   ```

3. **Create bead(s)** for your feedback:
   ```bash
   br create --actor $AGENT --title="..." --description="..." --type=bug --priority=2
   br create --actor $AGENT --title="..." --description="..." --type=feature --priority=3
   ```
   Include:
   - Reproduction steps (for bugs)
   - Context about what you were doing
   - Examples or test cases
   - Your use case (for feature requests)

4. **Post to the project channel** on botbus:
   ```bash
   bus send --agent $AGENT <project-name> "Filed N beads based on <context>: <bead-ids>. <summary> @<lead-agent>" -L mesh -L feedback
   ```
   Example:
   ```bash
   bus send --agent worker-alpha botty "Filed 7 beads based on e2e testing with botbox: bd-2rx (named keys), bd-2lj (record/replay), bd-142 (combined wait), bd-3qu (snapshot diff), bd-l96 (kill handling), bd-202 (assertions), bd-13p (script mode). Top priorities: named key sequences and record/replay. @botty-dev" -L mesh -L feedback
   ```

5. **Ensure the lead agent is running** so your report gets triaged:
   ```bash
   bus claims check --agent $AGENT "agent://<lead-agent>"
   ```
   If the claim is **not held** (lead agent is offline), spawn it:
   ```bash
   botty spawn --name <lead-agent> -- bash scripts/dev-loop.sh <project-name> <lead-agent>
   ```
   If `botty` is not available, skip this step — the report will be picked up
   when the lead agent starts its next session.

6. **Return to your work** — the lead agent will triage and respond.

## Notes

- The `#projects` channel contains the registry of all projects.
- Default lead agent naming: `<project>-dev` (e.g., `botty-dev`, `crit-dev`).
- Use `-L feedback` so the lead agent can filter for external reports.
- Always tag the lead agent with `@<lead-agent>` so they're notified.
