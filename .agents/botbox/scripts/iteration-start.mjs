#!/usr/bin/env bun
/**
 * iteration-start.mjs - Combined status for starting an iteration
 *
 * Outputs a compact summary of:
 * - Unread inbox messages
 * - Open beads (ready work)
 * - Reviews assigned to the agent
 *
 * Usage: bun .agents/botbox/scripts/iteration-start.mjs <project> <agent>
 */

import { execFileSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { parseArgs } from "node:util"

// ANSI colors
let C = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  dim: "\x1b[2m",
  cyan: "\x1b[36m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  red: "\x1b[31m",
  white: "\x1b[37m",
}

// Styled output helpers
let h1 = (s) => `${C.bold}${C.cyan}● ${s}${C.reset}`
let h2 = (s) => `${C.bold}${C.green}▸ ${s}${C.reset}`
let warn = (s) => `${C.bold}${C.yellow}▲ ${s}${C.reset}`
let error = (s) => `${C.bold}${C.red}✗ ${s}${C.reset}`
let hint = (s) => `${C.dim}→ ${s}${C.reset}`

// Parse args
let { values, positionals } = parseArgs({
  options: {
    help: { type: "boolean", short: "h" },
  },
  allowPositionals: true,
})

if (values.help) {
  console.log(`Usage: iteration-start.mjs [options] <project> <agent>

Combined status for starting an iteration. Shows:
- Unread inbox messages
- Open beads (ready work)
- Reviews assigned to the agent

Options:
  -h, --help    Show this help

Arguments:
  project       Project/channel name
  agent         Agent identity (e.g., myproject-dev)`)
  process.exit(0)
}

// Load project/agent from config if not provided
let PROJECT = positionals[0] || ""
let AGENT = positionals[1] || ""

if (!PROJECT || !AGENT) {
  if (existsSync(".botbox.json")) {
    try {
      let config = JSON.parse(readFileSync(".botbox.json", "utf-8"))
      let project = config.project || {}
      PROJECT = PROJECT || project.channel || project.name || ""
      AGENT = AGENT || project.defaultAgent || project.default_agent || ""
    } catch {
      // Ignore config errors
    }
  }
}

if (!PROJECT || !AGENT) {
  console.error("Error: project and agent required (provide as arguments or configure in .botbox.json)")
  console.error("Usage: iteration-start.mjs <project> <agent>")
  process.exit(1)
}

// Helper to run command and parse JSON
function runJson(cmd, args) {
  try {
    let output = execFileSync(cmd, args, {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    })
    return JSON.parse(output)
  } catch {
    return null
  }
}

// Helper to run command and get output
function run(cmd, args) {
  try {
    return execFileSync(cmd, args, {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim()
  } catch {
    return null
  }
}

console.log(h1(`Iteration Start: ${AGENT}`))
console.log()

// 1. Inbox messages
console.log(h2("Inbox"))
let inbox = runJson("bus", ["inbox", "--agent", AGENT, "--channels", PROJECT, "--format", "json"])
if (inbox && inbox.total_unread > 0) {
  console.log(`   ${inbox.total_unread} unread message(s)`)
  for (let channel of (inbox.channels || [])) {
    for (let msg of (channel.messages || []).slice(0, 5)) {
      let label = msg.label ? `[${msg.label}]` : ""
      let body = msg.body.length > 60 ? msg.body.substring(0, 60) + "..." : msg.body
      console.log(`   ${C.dim}${msg.agent}${C.reset} ${label}: ${body}`)
    }
  }
} else {
  console.log(`   ${C.dim}No unread messages${C.reset}`)
}
console.log()

// 2. Ready beads
console.log(h2("Ready Beads"))
let ready = runJson("maw", ["exec", "default", "--", "br", "ready", "--json"])
if (ready && Array.isArray(ready) && ready.length > 0) {
  console.log(`   ${ready.length} bead(s) ready`)
  for (let bead of ready.slice(0, 5)) {
    let priority = `P${bead.priority}`
    let owner = bead.owner ? `(${bead.owner})` : ""
    console.log(`   ${bead.id} ${priority} ${owner}: ${bead.title}`)
  }
  if (ready.length > 5) {
    console.log(`   ${C.dim}... and ${ready.length - 5} more${C.reset}`)
  }
} else {
  console.log(`   ${C.dim}No ready beads${C.reset}`)
}
console.log()

// 3. Reviews assigned to agent
console.log(h2("Pending Reviews"))
let reviews = runJson("maw", ["exec", "default", "--", "crit", "inbox", "--agent", AGENT, "--format", "json"])
if (reviews) {
  let awaiting = reviews.reviews_awaiting_vote || []
  let threads = reviews.threads_with_new_responses || []
  if (awaiting.length > 0 || threads.length > 0) {
    if (awaiting.length > 0) {
      console.log(`   ${awaiting.length} review(s) awaiting vote`)
      for (let r of awaiting.slice(0, 3)) {
        console.log(`   ${r.review_id}: ${r.title || r.description || "(no title)"}`)
      }
    }
    if (threads.length > 0) {
      console.log(`   ${threads.length} thread(s) with new responses`)
    }
  } else {
    console.log(`   ${C.dim}No pending reviews${C.reset}`)
  }
} else {
  console.log(`   ${C.dim}Could not fetch reviews${C.reset}`)
}
console.log()

// 4. Active claims
console.log(h2("Active Claims"))
let claims = runJson("bus", ["claims", "list", "--agent", AGENT, "--mine", "--format", "json"])
if (claims && claims.claims && claims.claims.length > 0) {
  // Filter out agent identity claims, keep resource claims
  let claimList = claims.claims.filter(c => {
    let patterns = c.patterns || []
    return !patterns.every(p => p.startsWith("agent://"))
  })
  if (claimList.length > 0) {
    console.log(`   ${claimList.length} active claim(s)`)
    for (let claim of claimList.slice(0, 5)) {
      let patterns = (claim.patterns || []).filter(p => !p.startsWith("agent://"))
      let expires = claim.expires_in_secs ? `(${Math.floor(claim.expires_in_secs / 60)}m left)` : ""
      for (let pattern of patterns) {
        console.log(`   ${pattern} ${expires}`)
      }
    }
  } else {
    console.log(`   ${C.dim}No resource claims${C.reset}`)
  }
} else {
  console.log(`   ${C.dim}No active claims${C.reset}`)
}
console.log()

// Summary hint
let hasInbox = inbox && inbox.total_unread > 0
let hasBeads = ready && Array.isArray(ready) && ready.length > 0
let hasReviews = reviews && ((reviews.reviews_awaiting_vote?.length || 0) + (reviews.threads_with_new_responses?.length || 0)) > 0

if (hasInbox) {
  console.log(hint(`Process inbox: bus inbox --agent ${AGENT} --channels ${PROJECT} --mark-read`))
} else if (hasReviews) {
  console.log(hint(`Start review: maw exec default -- crit inbox --agent ${AGENT}`))
} else if (hasBeads) {
  let top = ready[0]
  console.log(hint(`Claim top: maw exec default -- br update --actor ${AGENT} ${top.id} --status in_progress`))
} else {
  console.log(hint("No work pending"))
}
