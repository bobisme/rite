#!/usr/bin/env node
/**
 * respond.mjs - Conversational responder for mentions and questions
 *
 * Triggered by bus hooks when an agent is @mentioned. Parses the message
 * for question prefixes (q:, qq:, big q:, q(model):) to select the model,
 * responds, then waits for follow-up messages using `bus wait`.
 *
 * Environment (from hook):
 *   BOTBUS_CHANNEL - channel where the mention occurred
 *   BOTBUS_MESSAGE_ID - the triggering message ID
 *   BOTBUS_AGENT - the sender of the triggering message
 *
 * Usage: respond.mjs <project> <agent-name>
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

// --- Model mapping for question prefixes ---
const MODEL_MAP = {
  "qq:": "haiku",
  "q:": "sonnet",
  "big q:": "opus",
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
      AGENT = project.default_agent || ""
      DEFAULT_MODEL = responder.model || "sonnet"
      WAIT_TIMEOUT = responder.wait_timeout || 300
      CLAUDE_TIMEOUT = responder.timeout || 300
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
      help: { type: "boolean", short: "h" },
    },
    allowPositionals: true,
  })

  if (values.help) {
    console.log(`Usage: respond.mjs [options] <project> [agent-name]

Conversational responder for @mentions and questions.

Parses messages for question prefixes to select model:
  q:       -> sonnet (default)
  qq:      -> haiku (quick/cheap)
  big q:   -> opus (deep analysis)
  q(model): -> explicit model selection

Options:
  --model M         Default model (default: ${DEFAULT_MODEL})
  --timeout N       Claude timeout in seconds (default: ${CLAUDE_TIMEOUT})
  --wait-timeout N  Follow-up wait timeout in seconds (default: ${WAIT_TIMEOUT})
  -h, --help        Show this help

Arguments:
  project      Project name (default: from .botbox.json)
  agent-name   Agent identity (default: from .botbox.json or BOTBUS_AGENT env)

Environment (from hook):
  BOTBUS_CHANNEL    - channel where mention occurred
  BOTBUS_MESSAGE_ID - triggering message ID`)
    process.exit(0)
  }

  if (values.model) DEFAULT_MODEL = values.model
  if (values.timeout) CLAUDE_TIMEOUT = parseInt(values.timeout, 10)
  if (values["wait-timeout"]) WAIT_TIMEOUT = parseInt(values["wait-timeout"], 10)

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

// --- Parse question prefix and extract model ---
function parseQuestionPrefix(body) {
  // Check explicit q(model): syntax first
  let explicitMatch = body.match(/^q\((\w+)\):\s*/i)
  if (explicitMatch) {
    return {
      model: explicitMatch[1].toLowerCase(),
      question: body.slice(explicitMatch[0].length).trim(),
    }
  }

  // Check shortcut prefixes (order matters - check "big q:" before "q:")
  for (let [prefix, model] of Object.entries(MODEL_MAP).sort(
    (a, b) => b[0].length - a[0].length,
  )) {
    if (body.toLowerCase().startsWith(prefix)) {
      return {
        model,
        question: body.slice(prefix.length).trim(),
      }
    }
  }

  // No question prefix - return null (still respond, just use default model)
  return null
}

// --- Build response prompt ---
function buildPrompt(channel, senderAgent, messageBody, question) {
  let context = question
    ? `The user asked a question: "${question}"`
    : `The user mentioned you with message: "${messageBody}"`

  return `You are agent "${AGENT}" for project "${PROJECT}".

You were @mentioned in channel #${channel} by ${senderAgent}.
${context}

IMPORTANT:
- Use --agent ${AGENT} on ALL bus commands
- Keep responses concise and helpful
- If this is a question about the project, answer based on your knowledge
- If you need to check something (files, beads, etc.), do so
- After responding, the conversation may continue - keep context in mind

RESPOND using: bus send --agent ${AGENT} ${channel} "your response here" (no -L label needed)

Be helpful but brief. Do NOT create beads or workspaces - this is a conversational response, not a work task.

After posting your response, output: <promise>RESPONDED</promise>`
}

// --- Run agent via botbox run-agent ---
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

// --- Wait for follow-up message ---
async function waitForFollowUp(channel) {
  let args = [
    "wait",
    "--agent",
    AGENT,
    "--mention",
    "--channel",
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
    console.log(`bus wait: ${err.message.includes("timeout") ? "timeout" : err.message}`)
    return null // Timeout or error
  }
}

// --- Cleanup handler ---
async function cleanup() {
  console.log("Cleaning up...")
  try {
    await runCommand("bus", ["claims", "release", "--agent", AGENT, `respond://${AGENT}`])
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

// --- Main ---
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

  // Stake claim to prevent duplicate spawns
  let claimPattern = `respond://${AGENT}`
  try {
    await runCommand("bus", [
      "claims",
      "stake",
      "--agent",
      AGENT,
      claimPattern,
      "--ttl",
      `${WAIT_TIMEOUT + 60}`,
    ])
    console.log(`Claimed: ${claimPattern}`)
  } catch {
    // Claim already held - another agent is orchestrating, continue
    console.log(`Claim ${claimPattern} held by another agent, continuing`)
  }

  // Set status
  await runCommand("bus", [
    "statuses",
    "set",
    "--agent",
    AGENT,
    `Responding in #${channel}`,
    "--ttl",
    "10m",
  ])

  // Get unread @mentions from the channel
  let inboxResult
  try {
    inboxResult = await runCommand("bus", [
      "inbox",
      "--agent",
      AGENT,
      "--mentions",
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

  // Extract messages from inbox structure
  for (let ch of inbox.channels || []) {
    if (ch.channel === channel) {
      messages = ch.messages || []
      break
    }
  }

  if (messages.length === 0) {
    console.log("No unread @mentions in channel")
    await cleanup()
    process.exit(0)
  }

  // Find the triggering message by ID, or use most recent @mention
  let targetMessageId = process.env.BOTBUS_MESSAGE_ID
  let triggerMessage = targetMessageId
    ? messages.find((m) => m.id === targetMessageId)
    : messages[messages.length - 1]

  if (!triggerMessage) {
    // Fallback to most recent @mention
    triggerMessage = messages[messages.length - 1]
  }

  console.log(`Trigger: ${triggerMessage.agent}: ${triggerMessage.body.slice(0, 50)}...`)

  // Parse question prefix
  let parsed = parseQuestionPrefix(triggerMessage.body)
  let model = parsed?.model || DEFAULT_MODEL
  let question = parsed?.question || null

  console.log(`Model:   ${model}`)
  if (question) {
    console.log(`Question: ${question.slice(0, 50)}...`)
  }

  // Conversation loop
  let conversationCount = 0
  let maxConversations = 5
  let currentMessage = triggerMessage

  while (conversationCount < maxConversations) {
    conversationCount++
    console.log(`\n--- Response ${conversationCount}/${maxConversations} ---`)

    // Build and run prompt
    let prompt = buildPrompt(
      channel,
      currentMessage.agent,
      currentMessage.body,
      question,
    )

    try {
      await runClaude(prompt, model)
    } catch (err) {
      console.error("Error running Claude:", err.message)
      break
    }

    // Mark channel as read
    await runCommand("bus", ["mark-read", "--agent", AGENT, channel])

    // Wait for follow-up
    console.log(`\nWaiting ${WAIT_TIMEOUT}s for follow-up...`)

    // Refresh claim for the wait period
    try {
      await runCommand("bus", [
        "claims",
        "stake",
        "--agent",
        AGENT,
        `respond://${AGENT}`,
        "--ttl",
        `${WAIT_TIMEOUT + 60}`,
      ])
    } catch {}

    await runCommand("bus", [
      "statuses",
      "set",
      "--agent",
      AGENT,
      `Waiting for follow-up`,
      "--ttl",
      `${WAIT_TIMEOUT + 60}s`,
    ])

    let followUp = await waitForFollowUp(channel)
    if (!followUp) {
      console.log("No follow-up received, ending conversation")
      break
    }

    console.log(`Follow-up from ${followUp.agent}: ${followUp.body.slice(0, 50)}...`)
    currentMessage = followUp

    // Re-parse in case of new question prefix
    parsed = parseQuestionPrefix(followUp.body)
    if (parsed) {
      model = parsed.model
      question = parsed.question
    } else {
      question = null // Continuation, not new question
    }
  }

  await cleanup()
}

main().catch((err) => {
  console.error("Fatal error:", err)
  cleanup().finally(() => process.exit(1))
})
