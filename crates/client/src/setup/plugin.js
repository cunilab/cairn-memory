// cairn:continuity
import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { join } from "node:path"

const EXE = __CAIRN_EXE__
const SETUP_CONFIG = __CAIRN_SETUP_CONFIG__

function configFile() {
  const fromEnv = (process.env.CAIRN_CONFIG || "").trim()
  if (fromEnv) return fromEnv
  return SETUP_CONFIG
}

function sessionDirectory(session) {
  if (!session || typeof session !== "object") return ""
  const nested = session.data && session.data.location
  const location = session.location || nested
  if (location && typeof location.directory === "string") return location.directory
  return typeof session.directory === "string" ? session.directory : ""
}

function structuredInput(input) {
  const out = {}
  if (!input || typeof input !== "object") return out
  for (const key of ["file_path", "filePath", "path", "file", "command", "cmd"]) {
    const value = input[key]
    if (typeof value === "string" && value && value.length <= 512 && !value.includes("\n")) out[key] = value
  }
  return out
}

// The event is the first argument; the hook reads the rest of the payload from stdin.
function runHook(payload) {
  if (!payload || typeof payload.cwd !== "string" || !payload.cwd) return Promise.resolve("")
  const config = configFile()
  if (!existsSync(EXE)) return Promise.resolve("")
  return new Promise((resolve) => {
    const child = spawn(EXE, ["hook", payload.hook_event_name, "--agent", "opencode", "--config", config, "--cairn-managed"], { stdio: ["pipe", "pipe", "ignore"] })
    let out = ""
    const timer = setTimeout(() => { child.kill(); resolve("") }, 4000)
    child.stdout.on("data", (chunk) => { out += chunk })
    child.on("error", () => { clearTimeout(timer); resolve("") })
    child.on("close", () => {
      clearTimeout(timer)
      try {
        const parsed = JSON.parse(out)
        const text = parsed && parsed.hookSpecificOutput && parsed.hookSpecificOutput.additionalContext
        resolve(typeof text === "string" ? text : "")
      } catch (error) {
        resolve("")
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

async function directoryFor(ctx, sessionID) {
  if (!sessionID || !ctx.session || typeof ctx.session.get !== "function") return ""
  try {
    return sessionDirectory(await ctx.session.get({ sessionID }))
  } catch (error) {
    return ""
  }
}

function pushText(event, text) {
  if (!text || !event || !Array.isArray(event.system)) return
  event.system.push({ type: "text", text })
}

function ignored(directory) {
  return directory && existsSync(join(directory, ".cairnignore"))
}

export default {
  id: "cairn",
  async setup(ctx) {
    if (!ctx || !ctx.session || typeof ctx.session.hook !== "function") return
    const briefed = new Set()
    const pending = new Map()
    const queue = (sessionID, text) => {
      if (!sessionID || !text) return
      const list = pending.get(sessionID) || []
      list.push(text)
      pending.set(sessionID, list)
    }
    const take = (sessionID) => {
      const list = pending.get(sessionID) || []
      pending.delete(sessionID)
      return list.join("\n")
    }
    await ctx.session.hook("prompt", async (event) => {
      const sessionID = event && (event.sessionID || event.sessionId)
      const directory = await directoryFor(ctx, sessionID)
      const text = event && event.prompt && event.prompt.text
      if (!directory || typeof text !== "string" || !text) return
      await runHook({ hook_event_name: "UserPromptSubmit", session_id: sessionID, cwd: directory, prompt: text })
    })
    await ctx.session.hook("context", async (event) => {
      const sessionID = event && event.sessionID
      const directory = await directoryFor(ctx, sessionID)
      if (!directory || ignored(directory) || !event) return
      const parts = []
      if (!briefed.has(sessionID)) {
        briefed.add(sessionID)
        parts.push(await runHook({ hook_event_name: "SessionStart", session_id: sessionID, cwd: directory, source: "startup" }))
      }
      parts.push(take(sessionID))
      pushText(event, parts.filter(Boolean).join("\n"))
    })
    await ctx.session.hook("compaction", async (event) => {
      const sessionID = event && event.sessionID
      const directory = await directoryFor(ctx, sessionID)
      if (!directory || ignored(directory) || !event) return
      await runHook({ hook_event_name: "Stop", session_id: sessionID, cwd: directory })
      briefed.delete(sessionID)
      pushText(event, await runHook({ hook_event_name: "SessionStart", session_id: sessionID, cwd: directory, source: "compact" }))
    })
    if (ctx.tool && typeof ctx.tool.hook === "function") {
      try { await ctx.tool.hook("execute.after", async (event) => {
        const sessionID = event && (event.sessionID || event.sessionId)
        const directory = await directoryFor(ctx, sessionID)
        if (!directory || ignored(directory)) return
        const name = event && (event.tool || event.name || "")
        const input = structuredInput(event && (event.input || event.args || event.tool_input))
        const text = await runHook({ hook_event_name: "PostToolUse", session_id: sessionID, cwd: directory, tool_name: name, tool_input: input, tool_use_id: event && (event.callID || event.id) })
        queue(sessionID, text)
      }) } catch (error) {}
    }
    if (ctx.event && typeof ctx.event.subscribe === "function") {
      const controller = new AbortController()
      ;(async () => {
        for await (const item of ctx.event.subscribe({ signal: controller.signal })) {
          // 2.1.4 ends a turn with session.execution.succeeded and does not emit
          // session.idle. session.status remains for hosts that still send it.
          if (!item || (item.type !== "session.idle" && item.type !== "session.execution.succeeded" && item.type !== "session.status")) continue
          const info = item.data || item.properties || {}
          if (item.type === "session.status" && info.status && info.status !== "idle") continue
          const sessionID = info.sessionID || info.sessionId || info.id
          const located = item.location && item.location.directory
          const directory = located || info.directory || await directoryFor(ctx, sessionID)
          if (!directory || ignored(directory)) continue
          await runHook({ hook_event_name: "Stop", session_id: sessionID, cwd: directory })
        }
      })().catch(() => {})
    }
  },
}
