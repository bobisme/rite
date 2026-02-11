#!/usr/bin/env node
/**
 * respond.mjs - Universal message router for project channels
 *
 * THE single entrypoint for all project channel messages. Routes based on ! prefixes:
 *   !dev [msg]         → Create bead + spawn dev-loop
 *   !mission [desc]    → Create mission bead + spawn dev-loop with BOTBOX_MISSION
 *   !bead [desc]       → Create bead (with dedup via br search)
 *   !q [question]      → Answer with sonnet
 *   !qq [question]     → Answer with haiku
 *   !bigq [question]   → Answer with opus
 *   !q(model) [q]      → Answer with explicit model
 *   !oneshot [msg]      → Respond once, no follow-up loop
 *   No prefix          → Smart triage (chat vs question vs work)
 *
 * Backwards compatible with old q:/qq:/big q:/q(model): prefixes.
 *
 * Maintains a transcript buffer across conversation turns so each Claude call
 * gets full prior context. Mid-conversation escalation to dev-loop is supported.
 *
 * Environment (from hook):
 *   BOTBUS_CHANNEL    - channel where the message arrived
 *   BOTBUS_MESSAGE_ID - the triggering message ID
 *   BOTBUS_AGENT      - the sender of the triggering message
 *
 * Usage: respond.mjs [options] <project> [agent-name]
 */
import { spawn } from "child_process"
import { readFile } from "fs/promises"
import { existsSync } from "fs"
import { parseArgs } from "util"

// --- Defaults ---
let PROJECT = ""
let AGENT = ""
let DEFAULT_MODEL = "sonnet"
let WAIT_TIMEOUT = 300 // 5 minutes
let CLAUDE_TIMEOUT = 300 // 5 minutes for response
let MAX_CONVERSATIONS = 10 // max back-and-forth responses

// --- Transcript buffer ---
// Accumulates conversation history across bus wait iterations.
// Each entry: { role: "user"|"assistant", agent: string, body: string, timestamp: string }
let transcript = []

/**
 * Add an entry to the transcript buffer.
 * @param {"user"|"assistant"} role
 * @param {string} agent
 * @param {string} body
 */
function addToTranscript(role, agent, body) {
  transcript.push({
    role,
    agent,
    body,
    timestamp: new Date().toISOString(),
  })
}

/**
 * Format the transcript buffer as a readable conversation history for inclusion in prompts.
 * @returns {string}
 */
function formatTranscriptForPrompt() {
  if (transcript.length === 0) return ""

  let lines = ["## Conversation so far"]
  for (let entry of transcript) {
    let label = entry.role === "user" ? entry.agent : `${entry.agent} (you)`
    lines.push(`[${entry.timestamp}] ${label}: ${entry.body}`)
  }
  return lines.join("\n")
}

// --- Load config from .botbox.json ---
async function loadConfig() {
  if (existsSync(".botbox.json")) {
    try {
      let config = JSON.parse(await readFile(".botbox.json", "utf-8"))
      let project = config.project || {}
      let agents = config.agents || {}
      let responder = agents.responder || {}

      PROJECT = project.channel || project.name || ""
      AGENT = project.defaultAgent || project.default_agent || ""
      DEFAULT_MODEL = responder.model || "sonnet"
      WAIT_TIMEOUT = responder.wait_timeout || 300
      CLAUDE_TIMEOUT = responder.timeout || 300
      MAX_CONVERSATIONS = responder.max_conversations || 10
    } catch (err) {
      console.error("Warning: Failed to load .botbox.json:", err.message)
    }
  }
}

// --- Parse CLI args ---
function parseCliArgs() {
  let { values, positionals } = parseArgs({
    options: {
      model: { type: "string" },
      timeout: { type: "string" },
      "wait-timeout": { type: "string" },
      "max-conversations": { type: "string" },
      help: { type: "boolean", short: "h" },
    },
    allowPositionals: true,
  })

  if (values.help) {
    console.log(`Usage: respond.mjs [options] <project> [agent-name]

Universal message router for project channels.

Routes messages based on ! prefixes:
  !dev [msg]         Create bead + spawn dev-loop
  !mission [desc]    Create mission bead + spawn dev-loop
  !bead [desc]       Create bead (with dedup)
  !q [question]      Answer with sonnet (default)
  !qq [question]     Answer with haiku (quick/cheap)
  !bigq [question]   Answer with opus (deep analysis)
  !q(model) [q]      Answer with explicit model
  !oneshot [msg]     Respond once, no follow-up loop
  No prefix          Smart triage (chat vs question vs work)

Also accepts old-style prefixes: q:, qq:, big q:, q(model):

Options:
  --model M              Default model (default: ${DEFAULT_MODEL})
  --timeout N            Claude timeout in seconds (default: ${CLAUDE_TIMEOUT})
  --wait-timeout N       Follow-up wait timeout in seconds (default: ${WAIT_TIMEOUT})
  --max-conversations N  Max back-and-forth responses (default: ${MAX_CONVERSATIONS})
  -h, --help             Show this help

Arguments:
  project      Project name (default: from .botbox.json)
  agent-name   Agent identity (default: from .botbox.json or BOTBUS_AGENT env)

Environment (from hook):
  BOTBUS_CHANNEL    - channel where message arrived
  BOTBUS_MESSAGE_ID - triggering message ID`)
    process.exit(0)
  }

  if (values.model) DEFAULT_MODEL = values.model
  if (values.timeout) CLAUDE_TIMEOUT = parseInt(values.timeout, 10)
  if (values["wait-timeout"]) WAIT_TIMEOUT = parseInt(values["wait-timeout"], 10)
  if (values["max-conversations"])
    MAX_CONVERSATIONS = parseInt(values["max-conversations"], 10)

  if (positionals.length >= 1) PROJECT = positionals[0]
  if (positionals.length >= 2) AGENT = positionals[1]
}

// --- Helper: run command and get output ---
async function runCommand(cmd, args = []) {
  return new Promise((resolve, reject) => {
    let proc = spawn(cmd, args)
    let stdout = ""
    let stderr = ""

    proc.stdout?.on("data", (data) => (stdout += data))
    proc.stderr?.on("data", (data) => (stderr += data))

    proc.on("close", (code) => {
      if (code === 0) resolve({ stdout: stdout.trim(), stderr: stderr.trim() })
      else reject(new Error(`${cmd} exited with code ${code}: ${stderr}`))
    })
  })
}

// --- Helper: run command in default workspace (for br, bv) ---
function runInDefault(cmd, args = []) {
  return runCommand("maw", ["exec", "default", "--", cmd, ...args])
}

// ---------------------------------------------------------------------------
// Route message based on ! prefix
// ---------------------------------------------------------------------------

/**
 * @typedef {object} Route
 * @property {"dev"|"bead"|"mission"|"question"|"triage"|"oneshot"} type
 * @property {string} body
 * @property {string} [model]
 */

/**
 * Parse a message body and return a route describing how to handle it.
 * @param {string} body
 * @returns {Route}
 */
export function routeMessage(body) {
  let trimmed = body.trim()

  // --- ! prefix commands (new convention) ---

  // !oneshot [message] — respond once, no follow-up loop
  if (/^!oneshot\b/i.test(trimmed)) {
    return { type: "oneshot", body: trimmed.slice(8).trim() }
  }

  // !mission [description] — create mission bead + spawn dev-loop with BOTBOX_MISSION
  if (/^!mission\b/i.test(trimmed)) {
    return { type: "mission", body: trimmed.slice(8).trim() }
  }

  // !dev [message]
  if (/^!dev\b/i.test(trimmed)) {
    return { type: "dev", body: trimmed.slice(4).trim() }
  }

  // !bead [description]
  if (/^!bead\b/i.test(trimmed)) {
    return { type: "bead", body: trimmed.slice(5).trim() }
  }

  // !q(model) [question] — explicit model, must check before !q
  let bangExplicit = trimmed.match(/^!q\((\w+)\)\s*/i)
  if (bangExplicit) {
    return {
      type: "question",
      model: bangExplicit[1].toLowerCase(),
      body: trimmed.slice(bangExplicit[0].length).trim(),
    }
  }

  // !bigq [question]
  if (/^!bigq\b/i.test(trimmed)) {
    return { type: "question", model: "opus", body: trimmed.slice(5).trim() }
  }

  // !qq [question] — must check before !q
  if (/^!qq\b/i.test(trimmed)) {
    return { type: "question", model: "haiku", body: trimmed.slice(3).trim() }
  }

  // !q [question]
  if (/^!q\b/i.test(trimmed)) {
    return {
      type: "question",
      model: "sonnet",
      body: trimmed.slice(2).trim(),
    }
  }

  // --- Backwards compat: old colon-prefixed convention ---

  // q(model): [question]
  let oldExplicit = trimmed.match(/^q\((\w+)\):\s*/i)
  if (oldExplicit) {
    return {
      type: "question",
      model: oldExplicit[1].toLowerCase(),
      body: trimmed.slice(oldExplicit[0].length).trim(),
    }
  }

  // big q: [question] — check before q:
  let bigQ = trimmed.match(/^big q:\s*/i)
  if (bigQ) {
    return {
      type: "question",
      model: "opus",
      body: trimmed.slice(bigQ[0].length).trim(),
    }
  }

  // qq: [question] — check before q:
  let qqColon = trimmed.match(/^qq:\s*/i)
  if (qqColon) {
    return {
      type: "question",
      model: "haiku",
      body: trimmed.slice(qqColon[0].length).trim(),
    }
  }

  // q: [question]
  let qColon = trimmed.match(/^q:\s*/i)
  if (qColon) {
    return {
      type: "question",
      model: "sonnet",
      body: trimmed.slice(qColon[0].length).trim(),
    }
  }

  // --- No prefix → triage ---
  return { type: "triage", body: trimmed }
}

// ---------------------------------------------------------------------------
// Prompt builders
// ---------------------------------------------------------------------------

function buildQuestionPrompt(channel, message) {
  let transcriptBlock = formatTranscriptForPrompt()

  return `You are agent "${AGENT}" for project "${PROJECT}".

You received a message in channel #${channel} from ${message.agent}.
${transcriptBlock ? transcriptBlock + "\n\n" : ""}Current message: "${message.body}"

INSTRUCTIONS:
- Answer the question helpfully and concisely
- Use --agent ${AGENT} on ALL bus commands
- If you need to check files, beads, or code to answer, do so
- RESPOND using: bus send --agent ${AGENT} ${channel} "your response here"
- Do NOT create beads or workspaces — this is a conversation, not a work task
- If during the conversation you realize this is actually a bug or work item that needs
  immediate attention, output <escalate>brief description of the issue</escalate> AFTER
  posting your response. This will hand off to the dev-loop with full conversation context.

After posting your response, output: <promise>RESPONDED</promise>`
}

function buildTriagePrompt(channel, message) {
  return `You are agent "${AGENT}" for project "${PROJECT}".

You received a message in channel #${channel} from ${message.agent}:
"${message.body}"

Respond to this message. If it's clearly a work request (bug report, feature request, task,
"please fix/add/change X"), acknowledge it and output <escalate>one-line summary of the work</escalate>
so I can create a bead and spawn the dev-loop. Otherwise, just respond helpfully — I'll wait
for follow-ups automatically.

RULES:
- Use --agent ${AGENT} on ALL bus commands
- RESPOND using: bus send --agent ${AGENT} ${channel} "your response"
- Keep responses concise

After posting your response, output: <promise>RESPONDED</promise>`
}

// ---------------------------------------------------------------------------
// Run agent via botbox run-agent
// ---------------------------------------------------------------------------

async function runClaude(prompt, model) {
  return new Promise((resolve, reject) => {
    let args = ["run-agent", "claude", "-p", prompt]
    if (model) {
      args.push("-m", model)
    }
    args.push("-t", CLAUDE_TIMEOUT.toString())

    let proc = spawn("botbox", args)
    let output = ""

    proc.stdout?.on("data", (data) => {
      let chunk = data.toString()
      output += chunk
      process.stdout.write(chunk)
    })

    proc.stderr?.on("data", (data) => {
      process.stderr.write(data)
    })

    proc.on("close", (code) => {
      if (code === 0) {
        resolve({ output, code: 0 })
      } else {
        reject(new Error(`botbox run-agent exited with code ${code}`))
      }
    })

    proc.on("error", (err) => {
      reject(err)
    })
  })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Wait for a follow-up @mention on the channel. Returns message or null on timeout. */
async function waitForFollowUp(channel) {
  let args = [
    "wait",
    "--agent",
    AGENT,
    "--mentions",
    "--channels",
    channel,
    "--timeout",
    WAIT_TIMEOUT.toString(),
    "--format",
    "json",
  ]
  console.log(`Running: bus ${args.join(" ")}`)
  try {
    let result = await runCommand("bus", args)
    let parsed = JSON.parse(result.stdout)
    if (parsed.received && parsed.message) {
      console.log(`Follow-up received from ${parsed.message.agent}`)
      return parsed.message
    }
    return null
  } catch (err) {
    console.log(
      `bus wait: ${err.message.includes("timeout") ? "timeout" : err.message}`,
    )
    return null
  }
}

/**
 * Capture the agent's most recent message on the channel (the response it just posted).
 * Used to populate the transcript with what the agent actually said.
 * @param {string} channel
 * @returns {Promise<string|null>}
 */
async function captureAgentResponse(channel) {
  try {
    let result = await runCommand("bus", [
      "history",
      channel,
      "--from",
      AGENT,
      "-n",
      "1",
      "--format",
      "json",
    ])
    let parsed = JSON.parse(result.stdout)
    let messages = Array.isArray(parsed) ? parsed : (parsed.messages || [])
    if (messages.length > 0) {
      return messages[0].body
    }
  } catch {
    // Non-critical — transcript just won't have our response
  }
  return null
}

/** Refresh the agent claim TTL to prevent expiry during long conversations. */
async function refreshClaim() {
  try {
    await runCommand("bus", [
      "claims",
      "stake",
      "--agent",
      AGENT,
      `agent://${AGENT}`,
      "--ttl",
      `${WAIT_TIMEOUT + 120}`,
    ])
  } catch {
    // Best-effort
  }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/**
 * Handle !q / !qq / !bigq / !q(model) — question answering with conversation loop.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message - Trigger message object
 */
async function handleQuestion(route, channel, message) {
  addToTranscript("user", message.agent, message.body)
  let model = route.model || DEFAULT_MODEL
  let conversationCount = 0
  let currentMessage = message

  while (conversationCount < MAX_CONVERSATIONS) {
    conversationCount++
    console.log(`\n--- Response ${conversationCount}/${MAX_CONVERSATIONS} ---`)
    console.log(`Model: ${model}`)

    let prompt = buildQuestionPrompt(channel, currentMessage)

    try {
      let result = await runClaude(prompt, model)

      // Capture what the agent actually posted to the channel
      let response = await captureAgentResponse(channel)
      if (response) {
        addToTranscript("assistant", AGENT, response)
      }

      // Check for mid-conversation escalation
      let escalateMatch = result.output.match(/<escalate>([\s\S]*?)<\/escalate>/)
      if (escalateMatch) {
        let reason = escalateMatch[1].trim() || currentMessage.body
        console.log(`Escalation detected: ${reason}`)
        await handleDev(
          { type: "dev", body: reason },
          channel,
          currentMessage,
        )
        return
      }
    } catch (err) {
      console.error("Error running Claude:", err.message)
      break
    }

    // Mark channel as read
    try {
      await runCommand("bus", ["mark-read", "--agent", AGENT, channel])
    } catch {}

    // Wait for follow-up
    console.log(`\nWaiting ${WAIT_TIMEOUT}s for follow-up...`)
    await refreshClaim()
    await runCommand("bus", [
      "statuses",
      "set",
      "--agent",
      AGENT,
      "Waiting for follow-up",
      "--ttl",
      `${WAIT_TIMEOUT + 60}s`,
    ]).catch(() => {})

    let followUp = await waitForFollowUp(channel)
    if (!followUp) {
      console.log("No follow-up received, ending conversation")
      break
    }

    console.log(
      `Follow-up from ${followUp.agent}: ${followUp.body.slice(0, 80)}...`,
    )
    currentMessage = followUp

    // Re-route in case of new prefix (user might switch from !q to !dev mid-conversation)
    let reParsed = routeMessage(followUp.body)
    if (reParsed.type === "dev") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleDev(reParsed, channel, followUp)
      return
    }
    if (reParsed.type === "mission") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleMission(reParsed, channel, followUp)
      return
    }
    if (reParsed.type === "bead") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleBead(reParsed, channel, followUp)
      return
    }
    if (reParsed.type === "question") {
      model = reParsed.model // Switch model if new prefix
    }

    addToTranscript("user", followUp.agent, followUp.body)
  }
}

/**
 * Handle !bead — create a bead with dedup.
 * Returns the created bead ID or null.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message
 * @returns {Promise<string|null>}
 */
async function handleBead(route, channel, message) {
  if (!route.body) {
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      "Usage: !bead <description of what needs to be done>",
    ])
    return null
  }

  // Dedup: search for similar open beads
  let keywords = route.body
    .split(/\s+/)
    .filter((w) => w.length > 3)
    .slice(0, 5)
    .join(" ")
  if (keywords) {
    try {
      let result = await runInDefault("br", ["search", keywords])
      // br search output: "Found N issue(s) matching '...'" followed by bead lines
      if (result.stdout && !result.stdout.includes("Found 0")) {
        // Extract matches — lines containing bd-XXXX
        let matches = result.stdout
          .split("\n")
          .filter((l) => /bd-\w+/.test(l))
        if (matches.length > 0) {
          let firstMatch = matches[0].match(/bd-\w+/)
          let matchList = matches.slice(0, 3).join("\n")
          await runCommand("bus", [
            "send",
            "--agent",
            AGENT,
            channel,
            `Possible duplicates found:\n${matchList}\nUse \`br show <id>\` to check. Send \`!bead\` again with more specific wording to force-create.`,
          ])
          return firstMatch ? firstMatch[0] : null
        }
      }
    } catch {
      // Search failed — proceed with creation (fail open)
    }
  }

  // Create the bead — first line is title, rest is description
  let lines = route.body.split("\n")
  let title = lines[0].trim()
  if (title.length > 80) title = title.slice(0, 80).trim()
  let description = lines.length > 1 ? lines.slice(1).join("\n").trim() : title
  if (transcript.length > 0) {
    description +=
      "\n\n## Conversation context\n\n" + formatTranscriptForPrompt()
  }

  try {
    let result = await runInDefault("br", [
      "create",
      "--actor",
      AGENT,
      "--owner",
      AGENT,
      `--title=${title}`,
      `--description=${description}`,
      "--type=task",
      "--priority=2",
    ])
    let beadMatch = result.stdout.match(/bd-\w+/)
    let beadId = beadMatch ? beadMatch[0] : "unknown"
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      `Created ${beadId}: ${title}`,
      "-L",
      "feedback",
    ])
    return beadId
  } catch (err) {
    console.error("Error creating bead:", err.message)
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      `Failed to create bead: ${err.message}`,
    ])
    return null
  }
}

/**
 * Handle !dev — spawn dev-loop.
 * Also used for mid-conversation escalation (transcript will have context).
 * Does NOT create beads — use !bead for that. The dev-loop handles its own triage.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message
 */
async function handleDev(route, channel, message) {
  // Exec into dev-loop directly — we're already inside a botty PTY session,
  // so the dev-loop inherits it. Using botty spawn would kill our own session
  // (same --name) and orphan the child process.
  let scriptPath = ".agents/botbox/scripts/dev-loop.mjs"
  let args = ["bun", scriptPath, PROJECT, AGENT]
  console.log(`Exec into dev-loop: ${args.join(" ")}`)

  await runCommand("bus", [
    "send",
    "--agent",
    AGENT,
    channel,
    `Dev agent spawned — working on it.`,
    "-L",
    "spawn-ack",
  ]).catch(() => {})

  // Hand off to dev-loop with inherited stdio — this replaces our process
  let proc = spawn("bun", [scriptPath, PROJECT, AGENT], {
    stdio: "inherit",
    env: process.env,
  })
  let code = await new Promise((resolve) => {
    proc.on("close", (c) => resolve(c ?? 1))
  })
  process.exit(code)
}

/**
 * Handle !mission — create a mission bead and spawn dev-loop with BOTBOX_MISSION env var.
 * Missions are outcome-oriented beads with structured descriptions.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message
 */
async function handleMission(route, channel, message) {
  if (!route.body) {
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      "Usage: !mission <description of the desired outcome>",
    ])
    return
  }

  // Build title from first line (max 80 chars)
  let lines = route.body.split("\n")
  let title = lines[0].trim()
  if (title.length > 80) title = title.slice(0, 80).trim()

  // Build structured description
  let description
  if (lines.length > 1) {
    // Multi-line: use full body as description
    description = route.body.trim()
  } else {
    // Single line: add structured mission template
    description = `Outcome: ${route.body.trim()}\nSuccess metric: TBD\nConstraints: TBD\nStop criteria: TBD`
  }

  // Append transcript context if available
  if (transcript.length > 0) {
    description +=
      "\n\n## Conversation context\n\n" + formatTranscriptForPrompt()
  }

  // Create the mission bead
  let beadId
  try {
    let result = await runInDefault("br", [
      "create",
      "--actor",
      AGENT,
      "--owner",
      AGENT,
      `--title=${title}`,
      `--description=${description}`,
      "--labels",
      "mission",
      "--type=task",
      "--priority=2",
    ])
    let beadMatch = result.stdout.match(/bd-\w+/)
    beadId = beadMatch ? beadMatch[0] : "unknown"
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      `Mission created: ${beadId}: ${title}`,
      "-L",
      "feedback",
    ])
  } catch (err) {
    console.error("Error creating mission bead:", err.message)
    await runCommand("bus", [
      "send",
      "--agent",
      AGENT,
      channel,
      `Failed to create mission bead: ${err.message}`,
    ])
    return
  }

  // Spawn dev-loop with BOTBOX_MISSION env var
  let scriptPath = ".agents/botbox/scripts/dev-loop.mjs"
  let args = ["bun", scriptPath, PROJECT, AGENT]
  console.log(`Exec into dev-loop with mission ${beadId}: ${args.join(" ")}`)

  await runCommand("bus", [
    "send",
    "--agent",
    AGENT,
    channel,
    `Dev agent spawned for mission ${beadId}.`,
    "-L",
    "spawn-ack",
  ]).catch(() => {})

  // Hand off to dev-loop with BOTBOX_MISSION in env
  let env = { ...process.env, BOTBOX_MISSION: beadId }
  let proc = spawn("bun", [scriptPath, PROJECT, AGENT], {
    stdio: "inherit",
    env,
  })
  let code = await new Promise((resolve) => {
    proc.on("close", (c) => resolve(c ?? 1))
  })
  process.exit(code)
}

/**
 * Handle bare messages (no ! prefix) — smart triage via haiku.
 * Classifies as chat, question, or work and dispatches accordingly.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message
 */
async function handleTriage(route, channel, message) {
  console.log("Triage: classifying message...")
  addToTranscript("user", message.agent, message.body)

  let prompt = buildTriagePrompt(channel, message)

  try {
    let result = await runClaude(prompt, "haiku")

    // Capture what the agent responded
    let response = await captureAgentResponse(channel)
    if (response) {
      addToTranscript("assistant", AGENT, response)
    }

    // Check for escalation signal (work request)
    let escalateMatch = result.output.match(/<escalate>([\s\S]*?)<\/escalate>/)
    if (escalateMatch) {
      let reason = escalateMatch[1].trim() || route.body
      console.log(`Triage → work: "${reason}"`)
      await handleDev({ type: "dev", body: reason }, channel, message)
      return
    }

    // No escalation — always enter conversation mode so user can follow up
    console.log("Triage → responding, entering conversation mode")
    await handleQuestionFollowUpLoop(channel, message)
    return
  } catch (err) {
    console.error("Error in triage:", err.message)
  }
}

/**
 * Handle !oneshot — respond once with no follow-up loop.
 * Uses default model (sonnet). No transcript, no bus wait.
 * @param {Route} route
 * @param {string} channel
 * @param {any} message
 */
async function handleOneshot(route, channel, message) {
  let prompt = buildQuestionPrompt(channel, message)
  try {
    await runClaude(prompt, DEFAULT_MODEL)
  } catch (err) {
    console.error("Error running Claude:", err.message)
  }
  // Mark channel as read and exit — no follow-up loop
  try {
    await runCommand("bus", ["mark-read", "--agent", AGENT, channel])
  } catch {}
}

/**
 * After triage classifies a message as a question and already responded once,
 * enter the follow-up loop to continue the conversation.
 * @param {string} channel
 * @param {any} lastMessage
 */
async function handleQuestionFollowUpLoop(channel, lastMessage) {
  let conversationCount = 1 // Already responded once in triage
  let currentMessage = lastMessage

  while (conversationCount < MAX_CONVERSATIONS) {
    // Mark channel as read
    try {
      await runCommand("bus", ["mark-read", "--agent", AGENT, channel])
    } catch {}

    // Wait for follow-up
    console.log(`\nWaiting ${WAIT_TIMEOUT}s for follow-up...`)
    await refreshClaim()
    await runCommand("bus", [
      "statuses",
      "set",
      "--agent",
      AGENT,
      "Waiting for follow-up",
      "--ttl",
      `${WAIT_TIMEOUT + 60}s`,
    ]).catch(() => {})

    let followUp = await waitForFollowUp(channel)
    if (!followUp) {
      console.log("No follow-up received, ending conversation")
      break
    }

    console.log(
      `Follow-up from ${followUp.agent}: ${followUp.body.slice(0, 80)}...`,
    )
    currentMessage = followUp

    // Re-route in case of new prefix
    let reParsed = routeMessage(followUp.body)
    if (reParsed.type === "dev") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleDev(reParsed, channel, followUp)
      return
    }
    if (reParsed.type === "mission") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleMission(reParsed, channel, followUp)
      return
    }
    if (reParsed.type === "bead") {
      addToTranscript("user", followUp.agent, followUp.body)
      await handleBead(reParsed, channel, followUp)
      return
    }

    addToTranscript("user", followUp.agent, followUp.body)
    conversationCount++
    console.log(`\n--- Response ${conversationCount}/${MAX_CONVERSATIONS} ---`)

    let model =
      reParsed.type === "question" ? reParsed.model || DEFAULT_MODEL : DEFAULT_MODEL
    console.log(`Model: ${model}`)

    let prompt = buildQuestionPrompt(channel, currentMessage)
    try {
      let result = await runClaude(prompt, model)

      let response = await captureAgentResponse(channel)
      if (response) {
        addToTranscript("assistant", AGENT, response)
      }

      // Check for escalation
      let escalateMatch = result.output.match(/<escalate>([\s\S]*?)<\/escalate>/)
      if (escalateMatch) {
        let reason = escalateMatch[1].trim() || currentMessage.body
        console.log(`Escalation detected: ${reason}`)
        await handleDev(
          { type: "dev", body: reason },
          channel,
          currentMessage,
        )
        return
      }
    } catch (err) {
      console.error("Error running Claude:", err.message)
      break
    }
  }
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

async function cleanup() {
  console.log("Cleaning up...")
  try {
    await runCommand("bus", [
      "claims",
      "release",
      "--agent",
      AGENT,
      `agent://${AGENT}`,
    ])
  } catch {}
  try {
    await runCommand("bus", ["statuses", "clear", "--agent", AGENT])
  } catch {}
  console.log(`Cleanup complete for ${AGENT}.`)
}

process.on("SIGINT", async () => {
  await cleanup()
  process.exit(0)
})

process.on("SIGTERM", async () => {
  await cleanup()
  process.exit(0)
})

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  await loadConfig()
  parseCliArgs()

  // Get channel from env (set by hook)
  let channel = process.env.BOTBUS_CHANNEL
  if (!channel) {
    console.error("Error: BOTBUS_CHANNEL not set (should be set by hook)")
    process.exit(1)
  }

  // Use env agent if not specified
  if (!AGENT) {
    AGENT = process.env.BOTBUS_AGENT || `${PROJECT}-dev`
  }

  if (!PROJECT) {
    console.error("Error: Project name required")
    process.exit(1)
  }

  console.log(`Agent:   ${AGENT}`)
  console.log(`Project: ${PROJECT}`)
  console.log(`Channel: ${channel}`)

  // Set status
  await runCommand("bus", [
    "statuses",
    "set",
    "--agent",
    AGENT,
    `Routing message in #${channel}`,
    "--ttl",
    "10m",
  ]).catch(() => {})

  // Get the triggering message — prefer direct fetch by ID over inbox
  let targetMessageId = process.env.BOTBUS_MESSAGE_ID
  let triggerMessage = null

  if (targetMessageId) {
    try {
      let result = await runCommand("bus", [
        "messages",
        "get",
        targetMessageId,
        "--format",
        "json",
      ])
      triggerMessage = JSON.parse(result.stdout)
      console.log(`Fetched message ${targetMessageId} directly`)
    } catch (err) {
      console.error(
        `Warning: Could not fetch message ${targetMessageId}:`,
        err.message,
      )
    }
  }

  // Fall back to inbox if direct fetch failed or no message ID
  if (!triggerMessage) {
    let inboxResult
    try {
      inboxResult = await runCommand("bus", [
        "inbox",
        "--agent",
        AGENT,
        "--channels",
        channel,
        "--format",
        "json",
        "--mark-read",
      ])
    } catch (err) {
      console.error("Error reading inbox:", err.message)
      process.exit(1)
    }

    let inbox = JSON.parse(inboxResult.stdout || "{}")
    let messages = []

    for (let ch of inbox.channels || []) {
      if (ch.channel === channel) {
        messages = ch.messages || []
        break
      }
    }

    if (messages.length === 0) {
      console.log("No unread messages in channel and no message ID provided")
      await cleanup()
      process.exit(0)
    }

    triggerMessage = messages[messages.length - 1]
  }

  console.log(
    `Trigger: ${triggerMessage.agent}: ${triggerMessage.body.slice(0, 80)}...`,
  )

  // Route the message
  let route = routeMessage(triggerMessage.body)
  console.log(`Route:   ${route.type}${route.model ? ` (model: ${route.model})` : ""}`)

  // Dispatch to handler
  switch (route.type) {
    case "dev":
      await handleDev(route, channel, triggerMessage)
      break
    case "mission":
      await handleMission(route, channel, triggerMessage)
      break
    case "bead":
      await handleBead(route, channel, triggerMessage)
      break
    case "question":
      await handleQuestion(route, channel, triggerMessage)
      break
    case "triage":
      await handleTriage(route, channel, triggerMessage)
      break
    case "oneshot":
      await handleOneshot(route, channel, triggerMessage)
      break
  }

  await cleanup()
}

// Only run when executed directly (not when imported for testing)
let isMain =
  typeof import.meta.main !== "undefined"
    ? import.meta.main
    : process.argv[1]?.endsWith("respond.mjs")

if (isMain) {
  main().catch((err) => {
    console.error("Fatal error:", err)
    cleanup().finally(() => process.exit(1))
  })
}
