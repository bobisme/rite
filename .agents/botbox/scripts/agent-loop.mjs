#!/usr/bin/env node
import { spawn } from 'child_process';
import { readFile } from 'fs/promises';
import { existsSync } from 'fs';
import { parseArgs } from 'util';

// --- Defaults ---
let MAX_LOOPS = 20;
let LOOP_PAUSE = 2;
let CLAUDE_TIMEOUT = 600;
let MODEL = '';
let PROJECT = '';
let AGENT = '';
let PUSH_MAIN = false;

// --- Load config from .botbox.json ---
async function loadConfig() {
	if (existsSync('.botbox.json')) {
		try {
			const config = JSON.parse(await readFile('.botbox.json', 'utf-8'));
			const project = config.project || {};
			const agents = config.agents || {};
			const worker = agents.worker || {};

			// Project identity (can be overridden by CLI args)
			PROJECT = project.channel || project.name || '';
			// Workers get auto-generated names by default (AGENT stays empty)

			// Agent settings
			MODEL = worker.model || '';
			CLAUDE_TIMEOUT = worker.timeout || 600;
			PUSH_MAIN = config.pushMain || false;
		} catch (err) {
			console.error('Warning: Failed to load .botbox.json:', err.message);
		}
	}
}

// --- Parse CLI args ---
function parseCliArgs() {
	const { values, positionals } = parseArgs({
		options: {
			'max-loops': { type: 'string' },
			pause: { type: 'string' },
			model: { type: 'string' },
			help: { type: 'boolean', short: 'h' },
		},
		allowPositionals: true,
	});

	if (values.help) {
		console.log(`Usage: agent-loop.mjs [options] <project> [agent-name]

Worker agent. Picks one task per iteration, implements it, requests review,
and finishes. Sequential — one bead at a time.

Options:
  --max-loops N   Max iterations (default: ${MAX_LOOPS})
  --pause N       Seconds between iterations (default: ${LOOP_PAUSE})
  --model M       Model for the worker agent (default: system default)
  -h, --help      Show this help

Arguments:
  project         Project name (default: from .botbox.json)
  agent-name      Agent identity (default: auto-generated)`);
		process.exit(0);
	}

	if (values['max-loops']) MAX_LOOPS = parseInt(values['max-loops'], 10);
	if (values.pause) LOOP_PAUSE = parseInt(values.pause, 10);
	if (values.model) MODEL = values.model;

	// CLI args override config values
	if (positionals.length >= 1) {
		PROJECT = positionals[0];
	}
	if (positionals.length >= 2) {
		AGENT = positionals[1];
	}

	// Require project (either from CLI or config)
	if (!PROJECT) {
		console.error('Error: Project name required (provide as argument or configure in .botbox.json)');
		console.error('Usage: agent-loop.mjs [options] <project> [agent-name]');
		process.exit(1);
	}
}

// --- Helper: run command and get output ---
async function runCommand(cmd, args = []) {
	return new Promise((resolve, reject) => {
		const proc = spawn(cmd, args);
		let stdout = '';
		let stderr = '';

		proc.stdout?.on('data', (data) => (stdout += data));
		proc.stderr?.on('data', (data) => (stderr += data));

		proc.on('close', (code) => {
			if (code === 0) resolve({ stdout: stdout.trim(), stderr: stderr.trim() });
			else reject(new Error(`${cmd} exited with code ${code}: ${stderr}`));
		});
	});
}

// --- Helper: generate agent name if not provided ---
async function getAgentName() {
	if (AGENT) return AGENT;
	try {
		const { stdout } = await runCommand('bus', ['generate-name']);
		return stdout.trim();
	} catch (err) {
		console.error('Error generating agent name:', err.message);
		process.exit(1);
	}
}

// --- Helper: check if there is work ---
async function hasWork() {
	try {
		// Check claims
		const claimsResult = await runCommand('bus', [
			'claims',
			'list',
			'--agent',
			AGENT,
			'--mine',
			'--format',
			'json',
		]);
		const claims = JSON.parse(claimsResult.stdout || '{}');
		if (claims.claims && claims.claims.length > 0) return true;

		// Check inbox
		const inboxResult = await runCommand('bus', [
			'inbox',
			'--agent',
			AGENT,
			'--channels',
			PROJECT,
			'--count-only',
			'--format',
			'json',
		]);
		const inbox = JSON.parse(inboxResult.stdout || '{}');
		if (inbox.total_unread > 0) return true;

		// Check ready beads
		const readyResult = await runCommand('br', ['ready', '--json']);
		const ready = JSON.parse(readyResult.stdout || '[]');
		const readyCount = Array.isArray(ready) ? ready.length : ready.issues?.length || ready.beads?.length || 0;
		if (readyCount > 0) return true;

		return false;
	} catch (err) {
		console.error('Error checking for work:', err.message);
		return false;
	}
}

// --- Build worker prompt ---
function buildPrompt() {
	const pushMainStep = PUSH_MAIN
		? '\n   Push to GitHub: jj bookmark set main -r @- && jj git push (if fails, announce issue).'
		: '';

	return `You are worker agent "${AGENT}" for project "${PROJECT}".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Use --actor ${AGENT} on ALL mutating br commands (create, update, close, comments add, dep add, label add). Also use --owner ${AGENT} on br create and --author ${AGENT} on br comments add. Set BOTBOX_PROJECT=${PROJECT}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread botbus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.

Execute exactly ONE cycle of the worker loop. Complete one task (or determine there is no work),
then STOP. Do not start a second task — the outer loop handles iteration.

At the end of your work, output exactly one of these completion signals:
- <promise>COMPLETE</promise> if you completed a task or determined there is no work
- <promise>BLOCKED</promise> if you are stuck and cannot proceed

0. RESUME CHECK (do this FIRST):
   Run: bus claims list --agent ${AGENT} --mine
   If you hold a bead:// claim, you have an in-progress bead from a previous iteration.
   - Run: br comments <bead-id> to understand what was done before and what remains.
   - Look for workspace info in comments (workspace name and path).
   - If a "Review requested: <review-id>" comment exists:
     * Check review status: crit review <review-id>
     * If LGTM (approved): proceed to FINISH (step 7) — merge the review and close the bead.
     * If BLOCKED (changes requested): follow .agents/botbox/review-response.md to fix issues
       in the workspace, re-request review, then STOP this iteration.
     * If PENDING (no votes yet): STOP this iteration. Wait for the reviewer.
   - If no review comment (work was in progress when session ended):
     * Read the workspace code to see what's already done.
     * Complete the remaining work in the EXISTING workspace — do NOT create a new one.
     * After completing: br comments add --actor ${AGENT} --author ${AGENT} <id> "Resumed and completed: <what you finished>".
     * Then proceed to step 6 (REVIEW REQUEST) or step 7 (FINISH).
   If no active claims: proceed to step 1 (INBOX).

1. INBOX (do this before triaging):
   Run: bus inbox --agent ${AGENT} --channels ${PROJECT} --mark-read
   For each message:
   - Task request (-L task-request or asks for work): create a bead with br create.
   - Status check or question: reply on bus, do NOT create a bead.
   - Feedback (-L feedback): review referenced beads, reply with triage result.
   - Announcements from other agents ("Working on...", "Completed...", "online"): ignore, no action.
   - Duplicate of existing bead: do NOT create another bead, note it covers the request.

2. TRIAGE: Check br ready. If no ready beads and inbox created none, say "NO_WORK_AVAILABLE" and stop.
   GROOM each ready bead (br show <id>): ensure clear title, description with acceptance criteria
   and testing strategy, appropriate priority. Fix anything missing, comment what you changed.
   Use bv --robot-next to pick exactly one small task. If the task is large, break it down with
   br create + br dep add, then bv --robot-next again. If a bead is claimed
   (bus claims check --agent ${AGENT} "bead://${PROJECT}/<id>"), skip it.

3. START: br update --actor ${AGENT} <id> --status=in_progress.
   bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "<id>".
   Create workspace: run maw ws create --random. Note the workspace name AND absolute path
   from the output (e.g., name "frost-castle", path "/abs/path/.workspaces/frost-castle").
   Store the name as WS and the absolute path as WS_PATH.
   IMPORTANT: All file operations (Read, Write, Edit) must use the absolute WS_PATH.
   For bash commands: cd \$WS_PATH && <command>. For jj commands: maw ws jj \$WS <args>.
   Do NOT cd into the workspace and stay there — the workspace is destroyed during finish.
   bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>".
   br comments add --actor ${AGENT} --author ${AGENT} <id> "Started in workspace \$WS (\$WS_PATH)".
   bus statuses set --agent ${AGENT} "Working: <id>" --ttl 30m.
   Announce: bus send --agent ${AGENT} ${PROJECT} "Working on <id>: <title>" -L task-claim.

4. WORK: br show <id>, then implement the task in the workspace.
   Add at least one progress comment: br comments add --actor ${AGENT} --author ${AGENT} <id> "Progress: ...".

5. STUCK CHECK: If same approach tried twice, info missing, or tool fails repeatedly — you are
   stuck. br comments add --actor ${AGENT} --author ${AGENT} <id> "Blocked: <details>".
   bus statuses set --agent ${AGENT} "Blocked: <short reason>".
   bus send --agent ${AGENT} ${PROJECT} "Stuck on <id>: <reason>" -L task-blocked.
   br update --actor ${AGENT} <id> --status=blocked.
   Release: bus claims release --agent ${AGENT} "bead://${PROJECT}/<id>".
   Output: <promise>BLOCKED</promise>
   Stop this cycle.

6. REVIEW REQUEST:
   Describe the change: maw ws jj \$WS describe -m "<id>: <summary>".
   Create review: crit reviews create --agent ${AGENT} --title "<title>" --description "<summary>".
   Add bead comment: br comments add --actor ${AGENT} --author ${AGENT} <id> "Review requested: <review-id>, workspace: \$WS (\$WS_PATH)".
   bus statuses set --agent ${AGENT} "Review: <review-id>".
   Request security review (if project has security reviewer):
     - Assign: crit reviews request <review-id> --reviewers ${PROJECT}-security --agent ${AGENT}
     - Spawn via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
     (The @mention triggers the auto-spawn hook — without it, no reviewer spawns!)
   Do NOT close the bead. Do NOT merge the workspace. Do NOT release claims.
   Output: <promise>COMPLETE</promise>
   STOP this iteration. The reviewer will process the review.

7. FINISH (only reached after LGTM from step 0, or if no review needed):
   IMPORTANT: Run ALL finish commands from the project root, not from inside the workspace.
   If your shell is cd'd into .workspaces/, cd back to the project root first.
   If a review was conducted:
     crit reviews merge <review-id> --agent ${AGENT}.
   br comments add --actor ${AGENT} --author ${AGENT} <id> "Completed by ${AGENT}".
   br close --actor ${AGENT} <id> --reason="Completed" --suggest-next.
   maw ws merge \$WS --destroy (if conflict, preserve and announce).
   bus claims release --agent ${AGENT} --all.
   br sync --flush-only.${pushMainStep}
   bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done.
   Output: <promise>COMPLETE</promise>

Key rules:
- Exactly one small task per cycle.
- Always finish or release before stopping.
- If claim denied, pick something else.
- All bus and crit commands use --agent ${AGENT}.
- All file operations use the absolute workspace path from maw ws create output. Do NOT cd into the workspace and stay there.
- Run br commands (br update, br close, br comments, br sync) from the project root, NOT from .workspaces/WS/.
- If a tool behaves unexpectedly, report it: bus send --agent ${AGENT} ${PROJECT} "Tool issue: <details>" -L tool-issue.
- STOP after completing one task or determining no work. Do not loop.
- Always output <promise>COMPLETE</promise> or <promise>BLOCKED</promise> at the end.`;
}

// --- Run agent via botbox run-agent ---
async function runClaude(prompt) {
	return new Promise((resolve, reject) => {
		const args = ['run-agent', 'claude', '-p', prompt];
		if (MODEL) {
			args.push('-m', MODEL);
		}
		args.push('-t', CLAUDE_TIMEOUT.toString());

		const proc = spawn('botbox', args);
		let output = '';

		proc.stdout?.on('data', (data) => {
			const chunk = data.toString();
			output += chunk;
			process.stdout.write(chunk); // Pass through to stdout
		});

		proc.stderr?.on('data', (data) => {
			process.stderr.write(data); // Pass through to stderr
		});

		proc.on('close', (code) => {
			if (code === 0) {
				resolve({ output, code: 0 });
			} else {
				reject(new Error(`botbox run-agent exited with code ${code}`));
			}
		});

		proc.on('error', (err) => {
			reject(err);
		});
	});
}

// --- Cleanup handler ---
async function cleanup() {
	console.log('Cleaning up...');
	try {
		await runCommand('bus', ['statuses', 'clear', '--agent', AGENT]);
	} catch {}
	try {
		await runCommand('bus', ['claims', 'release', '--agent', AGENT, `agent://${AGENT}`]);
	} catch {}
	try {
		await runCommand('bus', ['claims', 'release', '--agent', AGENT, '--all']);
	} catch {}
	try {
		await runCommand('br', ['sync', '--flush-only']);
	} catch {}
	console.log(`Cleanup complete for ${AGENT}.`);
}

process.on('SIGINT', async () => {
	await cleanup();
	process.exit(0);
});

process.on('SIGTERM', async () => {
	await cleanup();
	process.exit(0);
});

// --- Main ---
async function main() {
	await loadConfig();
	parseCliArgs();

	AGENT = await getAgentName();

	console.log(`Agent:     ${AGENT}`);
	console.log(`Project:   ${PROJECT}`);
	console.log(`Max loops: ${MAX_LOOPS}`);
	console.log(`Pause:     ${LOOP_PAUSE}s`);
	console.log(`Model:     ${MODEL || 'system default'}`);

	// Confirm identity
	try {
		await runCommand('bus', ['whoami', '--agent', AGENT]);
	} catch (err) {
		console.error('Error confirming agent identity:', err.message);
		process.exit(1);
	}

	// Refresh or stake agent lease
	try {
		await runCommand('bus', ['claims', 'refresh', '--agent', AGENT, `agent://${AGENT}`]);
	} catch {
		try {
			await runCommand('bus', [
				'claims',
				'stake',
				'--agent',
				AGENT,
				`agent://${AGENT}`,
				'-m',
				`worker-loop for ${PROJECT}`,
			]);
		} catch {
			// Claim held by another agent - they're orchestrating, continue
			console.log(`Claim held by another agent, continuing`);
		}
	}

	// Announce
	await runCommand('bus', [
		'send',
		'--agent',
		AGENT,
		PROJECT,
		`Agent ${AGENT} online, starting worker loop`,
		'-L',
		'spawn-ack',
	]);

	// Set starting status
	await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Starting loop', '--ttl', '10m']);

	// Main loop
	for (let i = 1; i <= MAX_LOOPS; i++) {
		console.log(`\n--- Loop ${i}/${MAX_LOOPS} ---`);

		if (!(await hasWork())) {
			await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Idle']);
			console.log('No work available. Exiting cleanly.');
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`No work remaining. Agent ${AGENT} signing off.`,
				'-L',
				'agent-idle',
			]);
			break;
		}

		// Run Claude
		try {
			const prompt = buildPrompt();
			const result = await runClaude(prompt);

			// Check for completion signals
			if (result.output.includes('<promise>COMPLETE</promise>')) {
				console.log('✓ Task cycle complete');
			} else if (result.output.includes('<promise>BLOCKED</promise>')) {
				console.log('⚠ Agent blocked');
			} else {
				console.log('Warning: No completion signal found in output');
			}
		} catch (err) {
			console.error('Error running Claude:', err.message);

			// Check for fatal API errors and post to botbus
			const isFatalError =
				err.message.includes('API Error') ||
				err.message.includes('rate limit') ||
				err.message.includes('overloaded');

			if (isFatalError) {
				console.error('Fatal error detected, posting to botbus and exiting...');
				try {
					await runCommand('bus', [
						'send',
						'--agent',
						AGENT,
						PROJECT,
						`Worker error: ${err.message}. Agent ${AGENT} going offline.`,
						'-L',
						'agent-error',
					]);
				} catch {
					// Ignore bus errors during shutdown
				}
				break; // Exit loop on fatal error
			}

			// Handle timeout separately
			if (err.message.includes('Timeout')) {
				console.error('Claude timed out. Session may be stuck.');
			}
			// Continue to next iteration on non-fatal errors
		}

		if (i < MAX_LOOPS) {
			await new Promise((resolve) => setTimeout(resolve, LOOP_PAUSE * 1000));
		}
	}

	await cleanup();
}

main().catch((err) => {
	console.error('Fatal error:', err);
	cleanup().finally(() => process.exit(1));
});
