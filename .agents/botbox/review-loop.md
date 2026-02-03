# Review Loop

Review loop for reviewer agents. Process pending review requests and leave feedback.

Your identity is `$AGENT`. All bus commands must include `--agent $AGENT`. Run `bus whoami --agent $AGENT` first if you need to confirm the identity.

## Loop

1. Read new review requests:
   - `bus inbox --agent $AGENT --channel $BOTBOX_PROJECT -n 50`
   - `bus wait --agent $AGENT -L review-request -t 5` (optional)
2. Find open reviews: `crit reviews list --agent $AGENT --status=open --format=json`
3. For each review, gather context before commenting:
   a. Read the review and diff: `crit review <id>` and `crit diff <id>`
      - `crit review <id> --format=json` includes `workspace.path` for reading source files
   b. Read the full source files changed in the diff from the **workspace path** (e.g., `.workspaces/$WS/src/file.rs`), not project root
   c. Read project config (e.g., `Cargo.toml`) for edition and dependency versions
   d. Run static analysis (e.g., `cargo clippy 2>&1`) — cite warnings in your comments
   e. If unsure about framework or library behavior, use web search to verify before commenting
   f. **Cross-file consistency**: Compare functions that follow similar patterns across files. Do all handlers that validate input use the validated result consistently? Are security checks (auth, path validation, sanitization) applied uniformly? If one function does it right and another doesn't, that's a bug.
   g. **Boundary checks**: Trace each user-supplied value (query params, path params, headers, body fields) through to where it's used. Check arithmetic for edge cases: 0, 1, MAX, negative values, empty strings.
4. For each issue found, comment with a severity level:
   - **CRITICAL**: Security vulnerabilities, data loss, crashes in production
   - **HIGH**: Correctness bugs, race conditions, resource leaks
   - **MEDIUM**: Error handling gaps, missing validation at boundaries
   - **LOW**: Code quality, naming, structure
   - **INFO**: Suggestions, style preferences, minor improvements
   - `crit comment <id> "SEVERITY: <feedback>" --file <path> --line <line-or-range>`
5. Vote:
   - `crit block <id> --reason "..."` if any CRITICAL or HIGH issues exist
   - `crit lgtm <id>` if no CRITICAL or HIGH issues
6. Post a summary in the project channel and tag the author: `bus send --agent $AGENT $BOTBOX_PROJECT "..." -L mesh -L review-done`

Focus on security and correctness. Ground findings in evidence — compiler output, documentation, or source code — not assumptions about API behavior.

## Re-review

When re-review is requested after a block, the author's fixes live in their **workspace**, not on the main branch. The main branch still has the pre-fix code until merge.

1. Get the workspace path from `crit review <id> --format=json` (field: `workspace.path`). This is auto-detected from the change_id.
2. Read source files from the **workspace path** (e.g., `.workspaces/$WS/src/main.rs`), not from the project root.
3. Run static analysis in the workspace: `cd $WS_PATH && cargo clippy 2>&1`
4. Verify each fix against the original issue — read actual code, don't just trust thread replies.
5. If all issues are resolved: `crit lgtm <id>`. If issues remain: reply on the thread explaining what's still wrong.
