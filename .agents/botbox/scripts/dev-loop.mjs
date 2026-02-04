#!/usr/bin/env node
import { spawn } from 'child_process';
import { readFile, writeFile, stat } from 'fs/promises';
import { existsSync } from 'fs';
import { parseArgs } from 'util';

// --- Defaults ---
let MAX_LOOPS = 20;
let LOOP_PAUSE = 2;
let CLAUDE_TIMEOUT = 600;
let MODEL = '';
let WORKER_MODEL = '';
let PROJECT = '';
let AGENT = '';
let PUSH_MAIN = false;
let REVIEW = true;

// --- Load config from .botbox.json ---
async function loadConfig() {
	if (existsSync('.botbox.json')) {
		try {
			const config = JSON.parse(await readFile('.botbox.json', 'utf-8'));
			const project = config.project || {};
			const agents = config.agents || {};
			const dev = agents.dev || {};
			const worker = agents.worker || {};

			// Project identity (can be overridden by CLI args)
			PROJECT = project.channel || project.name || '';
			AGENT = project.default_agent || '';

			// Agent settings
			MODEL = dev.model || '';
			WORKER_MODEL = worker.model || '';
			CLAUDE_TIMEOUT = dev.timeout || 600;
			PUSH_MAIN = config.pushMain || false;
			REVIEW = config.review ?? true;
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
			review: { type: 'boolean' },
			help: { type: 'boolean', short: 'h' },
		},
		allowPositionals: true,
	});

	if (values.help) {
		console.log(`Usage: dev-loop.mjs [options] <project> [agent-name]

Lead dev agent. Triages inbox, dispatches work to multiple workers in parallel
when appropriate, monitors progress, merges completed work.

Options:
  --max-loops N   Max iterations (default: ${MAX_LOOPS})
  --pause N       Seconds between iterations (default: ${LOOP_PAUSE})
  --model M       Model for the lead dev (default: system default)
  --review        Enable code review (default: ${REVIEW})
  --no-review     Disable code review
  -h, --help      Show this help

Arguments:
  project         Project name (default: from .botbox.json)
  agent-name      Agent identity (default: from .botbox.json or auto-generated)`);
		process.exit(0);
	}

	if (values['max-loops']) MAX_LOOPS = parseInt(values['max-loops'], 10);
	if (values.pause) LOOP_PAUSE = parseInt(values.pause, 10);
	if (values.model) MODEL = values.model;
	if (values.review !== undefined) REVIEW = values.review;

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
		console.error('Usage: dev-loop.mjs [options] <project> [agent-name]');
		process.exit(1);
	}
}

// --- Helper: get commits on main since origin ---
async function getCommitsSinceOrigin() {
	try {
		const { stdout } = await runCommand('jj', [
			'log',
			'-r',
			'main@origin..main',
			'--no-graph',
			'--template',
			'commit_id.short() ++ " " ++ description.first_line() ++ "\\n"',
		]);
		return stdout.trim().split('\n').filter(Boolean);
	} catch {
		return [];
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
		// Check claims (dispatched workers or in-progress beads)
		const claimsResult = await runCommand('bus', [
			'claims',
			'--agent',
			AGENT,
			'list',
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

// --- Read previous iteration summary ---
async function readLastIteration() {
	const path = '.agents/botbox/last-iteration.txt';
	if (!existsSync(path)) return null;

	try {
		const content = await readFile(path, 'utf-8');
		const stats = await stat(path);
		const ageMs = Date.now() - stats.mtime.getTime();
		const ageMinutes = Math.floor(ageMs / 60000);
		const ageHours = Math.floor(ageMinutes / 60);
		const ageStr = ageHours > 0 ? `${ageHours}h ago` : `${ageMinutes}m ago`;
		return { content: content.trim(), age: ageStr };
	} catch {
		return null;
	}
}

// --- Build dev lead prompt ---
function buildPrompt(lastIteration) {
	const pushMainStep = PUSH_MAIN
		? '\n  14. Push to GitHub: jj bookmark set main -r @- && jj git push (if fails, announce issue).'
		: '';

	const reviewInstructions = REVIEW ? 'REVIEW is true' : 'REVIEW is false';

	const previousContext = lastIteration
		? `\n\n## PREVIOUS ITERATION (${lastIteration.age}, may be stale)\n\n${lastIteration.content}\n`
		: '';

	return `You are lead dev agent "${AGENT}" for project "${PROJECT}".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Use --actor ${AGENT} on ALL mutating br commands. Use --author ${AGENT} on br comments add. Set BOTBOX_PROJECT=${PROJECT}. ${reviewInstructions}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread botbus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.
${previousContext}
Execute exactly ONE dev cycle. Triage inbox, assess ready beads, either work on one yourself
or dispatch multiple workers in parallel, monitor progress, merge results. Then STOP.

At the end of your work, output:
1. A summary for the next iteration: <iteration-summary>Brief summary of what you did: beads worked on, workers dispatched, reviews processed, etc.</iteration-summary>
2. Completion signal:
   - <promise>COMPLETE</promise> if you completed work or determined no work available
   - <promise>END_OF_STORY</promise> if iteration done but more work remains

## 1. RESUME CHECK (do this FIRST)

Run: bus claims list --agent ${AGENT} --mine

If you hold any claims:
- bead:// claim with review comment: Check crit review status. If LGTM, proceed to merge/finish.
- bead:// claim without review: Complete the work, then review or finish.
- workspace:// claims: These are dispatched workers. Skip to step 6 (MONITOR).

If no claims: proceed to step 2 (INBOX).

## 2. INBOX

Run: bus inbox --agent ${AGENT} --channels ${PROJECT} --mark-read

Process each message:
- Task requests (-L task-request): create beads with br create
- Status/questions: reply on bus
- Feedback (-L feedback): triage and respond
- Announcements ("Working on...", "Completed...", "online"): ignore, no action
- Duplicate requests: note existing bead, don't create another

## 3. TRIAGE

Run: br ready --json

Count ready beads. If 0 and inbox created none: output <promise>COMPLETE</promise> and stop.

GROOM each ready bead:
- br show <id> — ensure clear title, description, acceptance criteria, priority
- Comment what you changed: br comments add --actor ${AGENT} --author ${AGENT} <id> "..."
- If bead is claimed (check bus claims), skip it

Assess bead count:
- 0 ready beads (but dispatched workers pending): just monitor, skip to step 6.
- 1 ready bead: do it yourself sequentially (follow steps 4a below).
- 2+ ready beads: dispatch workers in parallel (follow steps 4b below).

## 4a. SEQUENTIAL (1 bead — do it yourself)

Same as the standard worker loop:
1. br update --actor ${AGENT} <id> --status=in_progress
2. bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "<id>"
3. maw ws create --random — note workspace NAME and absolute PATH
4. bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>"
5. br comments add --actor ${AGENT} --author ${AGENT} <id> "Started in workspace \$WS (\$WS_PATH)"
6. bus statuses set --agent ${AGENT} "Working: <id>" --ttl 30m
7. Announce: bus send --agent ${AGENT} ${PROJECT} "Working on <id>: <title>" -L task-claim
8. Implement the task. All file operations use absolute WS_PATH.
   For jj: maw ws jj \$WS <args>. Do NOT cd into workspace and stay there.
9. br comments add --actor ${AGENT} --author ${AGENT} <id> "Progress: ..."
10. Describe: maw ws jj \$WS describe -m "<id>: <summary>"

If REVIEW is true:
  11. Create review: crit reviews create --agent ${AGENT} --title "<title>" --description "<summary>"
  12. br comments add --actor ${AGENT} --author ${AGENT} <id> "Review requested: <review-id>, workspace: \$WS (\$WS_PATH)"
  13. bus statuses set --agent ${AGENT} "Review: <review-id>"
  14. Request security review (if project has security reviewer):
      - Assign: crit reviews request <review-id> --reviewers ${PROJECT}-security --agent ${AGENT}
      - Spawn via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
      (The @mention triggers the auto-spawn hook — without it, no reviewer spawns!)
  15. STOP this iteration — wait for reviewer.

If REVIEW is false:
  11. Merge: maw ws merge \$WS --destroy
  12. br close --actor ${AGENT} <id> --reason="Completed"
  13. bus claims release --agent ${AGENT} --all
  14. br sync --flush-only${pushMainStep}
  ${PUSH_MAIN ? '15' : '14'}. bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done

## 4b. PARALLEL DISPATCH (2+ beads)

For EACH independent ready bead, assess and dispatch:

### Model Selection
Read each bead (br show <id>) and select a model based on complexity:
- **${WORKER_MODEL || 'default'}**: Use for most tasks unless signals suggest otherwise.
- **haiku**: Clear acceptance criteria, small scope (<~50 lines), well-groomed. E.g., add endpoint, fix typo, update config.
- **sonnet**: Multiple files, design decisions, moderate complexity. E.g., refactor module, add feature with tests.
- **opus**: Deep debugging, architecture changes, subtle correctness issues. E.g., fix race condition, redesign data flow.

### For each bead being dispatched:
1. maw ws create --random — note NAME and PATH
2. bus generate-name — get a worker identity
3. br update --actor ${AGENT} <id> --status=in_progress
4. bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "dispatched to <worker-name>"
5. bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>"
6. br comments add --actor ${AGENT} --author ${AGENT} <id> "Dispatched worker <worker-name> (model: <model>) in workspace \$WS (\$WS_PATH)"
7. bus statuses set --agent ${AGENT} "Dispatch: <id>" --ttl 5m
8. bus send --agent ${AGENT} ${PROJECT} "Dispatching <worker-name> for <id>: <title>" -L task-claim

DO NOT actually spawn background processes — that's handled by bash/botty. Instead, just note:
"Workers would be spawned here in production. For now, skip to monitoring."

## 5. MONITOR (if workers are dispatched)

Check for completion messages:
- bus inbox --agent ${AGENT} --channels ${PROJECT} -n 20
- Look for task-done messages from workers
- Check workspace status: maw ws list

For each completed worker:
- Read their progress comments: br comments <id>
- Verify the work looks reasonable (spot check key files)

## 6. FINISH (merge completed work)

For each completed bead with a workspace:

If REVIEW is true:
  1. crit reviews create --agent ${AGENT} --title "<title>" --description "<summary of changes>"
  2. br comments add --actor ${AGENT} --author ${AGENT} <id> "Review requested: <review-id>"
  3. Request security review (if project has security reviewer):
     - Assign: crit reviews request <review-id> --reviewers ${PROJECT}-security --agent ${AGENT}
     - Spawn via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
     (The @mention triggers the auto-spawn hook — without it, no reviewer spawns!)
  4. STOP — wait for reviewer

If REVIEW is false:
  1. maw ws merge \$WS --destroy
  2. br close --actor ${AGENT} <id>
  3. br sync --flush-only${pushMainStep}
  4. bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done

After finishing all ready work:
  bus claims release --agent ${AGENT} --all
  Output: <promise>END_OF_STORY</promise> if more beads remain, else <promise>COMPLETE</promise>

Key rules:
- Triage first, then decide: sequential vs parallel
- Monitor dispatched workers, merge when ready
- All bus/crit commands use --agent ${AGENT}
- For parallel dispatch, note limitations of this prompt-based approach
- Output completion signal at end`;
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
	console.log(`Review:    ${REVIEW}`);

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
				`dev-loop for ${PROJECT}`,
			]);
		} catch (err) {
			console.log(`Claim denied. Agent ${AGENT} is already running.`);
			process.exit(0);
		}
	}

	// Announce
	await runCommand('bus', [
		'send',
		'--agent',
		AGENT,
		PROJECT,
		`Dev agent ${AGENT} online, starting dev loop`,
		'-L',
		'spawn-ack',
	]);

	// Set starting status
	await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Starting loop', '--ttl', '10m']);

	// Capture baseline commits for release tracking
	const baselineCommits = await getCommitsSinceOrigin();

	// Main loop
	for (let i = 1; i <= MAX_LOOPS; i++) {
		console.log(`\n--- Dev loop ${i}/${MAX_LOOPS} ---`);

		if (!(await hasWork())) {
			await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Idle']);
			console.log('No work available. Exiting cleanly.');
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`No work remaining. Dev agent ${AGENT} signing off.`,
				'-L',
				'agent-idle',
			]);
			break;
		}

		// Run Claude
		try {
			const lastIteration = await readLastIteration();
			const prompt = buildPrompt(lastIteration);
			const result = await runClaude(prompt);

			// Check for completion signals
			if (result.output.includes('<promise>COMPLETE</promise>')) {
				console.log('✓ Dev cycle complete - no more work');
				break;
			} else if (result.output.includes('<promise>END_OF_STORY</promise>')) {
				console.log('✓ Iteration complete - more work remains');
			} else {
				console.log('Warning: No completion signal found in output');
			}

			// Extract and save iteration summary for next time
			try {
				const summaryMatch = result.output.match(/<iteration-summary>([\s\S]*?)<\/iteration-summary>/);
				if (summaryMatch) {
					await writeFile('.agents/botbox/last-iteration.txt', summaryMatch[1].trim());
				}
			} catch (err) {
				console.error('Warning: Failed to save iteration summary:', err.message);
			}
		} catch (err) {
			console.error('Error running Claude:', err.message);
			// Continue to next iteration on error
		}

		if (i < MAX_LOOPS) {
			await new Promise((resolve) => setTimeout(resolve, LOOP_PAUSE * 1000));
		}
	}

	// Show what landed since session start (for release decisions)
	const finalCommits = await getCommitsSinceOrigin();
	const newCommits = finalCommits.filter((c) => !baselineCommits.includes(c));
	if (newCommits.length > 0) {
		console.log('\n--- Commits landed this session ---');
		for (const commit of newCommits) {
			console.log(`  ${commit}`);
		}
		console.log('\nIf any are user-visible (feat/fix), consider a release.');
	}

	await cleanup();
}

main().catch((err) => {
	console.error('Fatal error:', err);
	cleanup().finally(() => process.exit(1));
});
