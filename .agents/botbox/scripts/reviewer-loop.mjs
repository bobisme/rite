#!/usr/bin/env node
import { spawn } from 'child_process';
import { readFile } from 'fs/promises';
import { existsSync, readFileSync } from 'fs';
import { join } from 'path';
import { parseArgs } from 'util';

// --- Inline prompt utilities (scripts must be self-contained) ---

/**
 * Derive the reviewer role from an agent name.
 * e.g., "myproject-security" -> "security", "myproject-dev" -> null
 */
function deriveRoleFromAgentName(agentName, knownRoles = ['security']) {
	for (const role of knownRoles) {
		if (agentName.endsWith(`-${role}`)) {
			return role;
		}
	}
	return null;
}

/**
 * Get the prompt name for a reviewer based on role.
 */
function getReviewerPromptName(role) {
	if (role) {
		return `reviewer-${role}`;
	}
	return 'reviewer';
}

/**
 * Load a prompt template and substitute variables.
 */
function loadPrompt(promptName, variables, promptsDir) {
	const filePath = join(promptsDir, `${promptName}.md`);

	if (!existsSync(filePath)) {
		throw new Error(`Prompt template not found: ${filePath}`);
	}

	let template = readFileSync(filePath, 'utf-8');

	// Simple {{VARIABLE}} substitution
	for (const [key, value] of Object.entries(variables)) {
		const pattern = new RegExp(`\\{\\{${key}\\}\\}`, 'g');
		template = template.replace(pattern, value);
	}

	return template;
}

// --- Defaults ---
let MAX_LOOPS = 20;
let LOOP_PAUSE = 2;
let CLAUDE_TIMEOUT = 600;
let MODEL = '';
let PROJECT = '';
let AGENT = '';

// --- Load config from .botbox.json ---
async function loadConfig() {
	if (existsSync('.botbox.json')) {
		try {
			const config = JSON.parse(await readFile('.botbox.json', 'utf-8'));
			const project = config.project || {};
			const agents = config.agents || {};
			const reviewer = agents.reviewer || {};

			// Project identity (can be overridden by CLI args)
			PROJECT = project.channel || project.name || '';
			// Reviewer agent name is typically passed via CLI (e.g., maw-security)

			// Agent settings
			MODEL = reviewer.model || '';
			MAX_LOOPS = reviewer.max_loops || 20;
			LOOP_PAUSE = reviewer.pause || 2;
			CLAUDE_TIMEOUT = reviewer.timeout || 600;
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
		console.log(`Usage: reviewer-loop.mjs [options] <project> <agent-name>

Reviewer agent. Picks one open review per iteration, reads the diff,
leaves comments, and votes LGTM or BLOCKED.

Options:
  --max-loops N   Max iterations (default: ${MAX_LOOPS})
  --pause N       Seconds between iterations (default: ${LOOP_PAUSE})
  --model M       Model for the reviewer agent (default: ${MODEL || 'opus'})
  -h, --help      Show this help

Arguments:
  project         Project name (default: from .botbox.json)
  agent-name      Agent identity (required - determines reviewer role)`);
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
	} else if (positionals.length === 1) {
		// If only one arg provided, treat it as agent name (project from config)
		AGENT = positionals[0];
	}

	// Require project
	if (!PROJECT) {
		console.error('Error: Project name required (provide as argument or configure in .botbox.json)');
		console.error('Usage: reviewer-loop.mjs [options] [project] <agent-name>');
		process.exit(1);
	}

	// Require agent name
	if (!AGENT) {
		console.error('Error: Agent name required (determines reviewer role)');
		console.error('Usage: reviewer-loop.mjs [options] [project] <agent-name>');
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

// --- Helper: check if there are reviews needing attention ---
// Returns { hasWork: boolean, inbox: object }
async function findWork() {
	try {
		// crit inbox --all-workspaces searches both repo root and all jj workspaces
		// Shows only reviews awaiting this reviewer's response:
		// - Reviews where reviewer is assigned but hasn't voted
		// - Reviews that were re-requested after voting
		// Reviews disappear from inbox after voting until re-requested.
		const result = await runCommand('crit', [
			'inbox',
			'--agent',
			AGENT,
			'--all-workspaces',
			'--format',
			'json',
		]);
		const inbox = JSON.parse(result.stdout || '{}');
		const hasReviews =
			(inbox.reviews_awaiting_vote && inbox.reviews_awaiting_vote.length > 0) ||
			(inbox.threads_with_new_responses && inbox.threads_with_new_responses.length > 0);

		return {
			hasWork: hasReviews,
			inbox,
		};
	} catch (err) {
		console.error('Error finding work:', err.message);
		return { hasWork: false, inbox: {} };
	}
}

// --- Build reviewer prompt ---
function buildPrompt() {
	// Derive role from agent name (e.g., "myproject-security" -> "security")
	const role = deriveRoleFromAgentName(AGENT);
	const promptName = getReviewerPromptName(role);

	// Use project-local prompts
	const promptsDir = join(process.cwd(), '.agents', 'botbox', 'prompts');

	let basePrompt;
	try {
		basePrompt = loadPrompt(promptName, { AGENT, PROJECT }, promptsDir);
	} catch (err) {
		// Fall back to base reviewer if specialized prompt not found
		if (role && promptName !== 'reviewer') {
			console.warn(`Warning: ${promptName}.md not found, using base reviewer prompt`);
			try {
				basePrompt = loadPrompt('reviewer', { AGENT, PROJECT }, promptsDir);
			} catch {
				// If even base fails, throw original error
				throw err;
			}
		} else {
			throw err;
		}
	}

	return basePrompt;
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
		await runCommand('bus', [
			'send',
			'--agent',
			AGENT,
			PROJECT,
			`Reviewer ${AGENT} signing off.`,
			'-L',
			'agent-idle',
		]);
	} catch {}
	try {
		await runCommand('bus', ['statuses', 'clear', '--agent', AGENT]);
	} catch {}
	try {
		await runCommand('bus', ['claims', 'release', '--agent', AGENT, `agent://${AGENT}`]);
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

	console.log(`Reviewer:  ${AGENT}`);
	console.log(`Project:   ${PROJECT}`);
	console.log(`Max loops: ${MAX_LOOPS}`);
	console.log(`Pause:     ${LOOP_PAUSE}s`);
	console.log(`Model:     ${MODEL || 'opus'}`);

	// Confirm identity
	try {
		await runCommand('bus', ['whoami', '--agent', AGENT]);
	} catch (err) {
		console.error('Error confirming agent identity:', err.message);
		process.exit(1);
	}

	// Try to refresh claim, otherwise stake
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
				`reviewer-loop for ${PROJECT}`,
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
		`Reviewer ${AGENT} online, starting review loop`,
		'-L',
		'spawn-ack',
	]);

	// Set starting status
	await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Starting loop', '--ttl', '10m']);

	// Main loop
	for (let i = 1; i <= MAX_LOOPS; i++) {
		console.log(`\n--- Review loop ${i}/${MAX_LOOPS} ---`);

		const work = await findWork();
		if (!work.hasWork) {
			await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Idle']);
			console.log('No reviews pending. Exiting cleanly.');
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`No reviews pending. Reviewer ${AGENT} signing off.`,
				'-L',
				'agent-idle',
			]);
			break;
		}

		// Log what's pending
		const reviewCount = work.inbox.reviews_awaiting_vote?.length || 0;
		const threadCount = work.inbox.threads_with_new_responses?.length || 0;
		console.log(`  ${reviewCount} reviews awaiting vote, ${threadCount} threads with responses`);

		// Run Claude
		try {
			const prompt = buildPrompt();
			const result = await runClaude(prompt);

			// Check for completion signals
			if (result.output.includes('<promise>COMPLETE</promise>')) {
				console.log('✓ Review cycle complete');
			} else if (result.output.includes('<promise>BLOCKED</promise>')) {
				console.log('⚠ Reviewer blocked');
			} else {
				console.log('Warning: No completion signal found in output');
			}
		} catch (err) {
			console.error('Error running Claude:', err.message);
			// Continue to next iteration on error
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
