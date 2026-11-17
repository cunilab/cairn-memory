import { spawn } from "node:child_process";
import { mkdir, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = process.env.CAIRN_GRAPH_SHOTS || join(tmpdir(), "cairn-stage8-graph");
const password = "correct horse battery staple";

function loadPlaywright() {
  const require = createRequire(import.meta.url);
  try {
    return require("playwright");
  } catch {
    throw new Error("playwright is not on NODE_PATH. Point NODE_PATH at an existing install.");
  }
}

const logs = [];
function listen(child, label) {
  const remember = (chunk) => {
    for (const line of String(chunk).split("\n")) {
      if (!line) continue;
      logs.push(`${label}: ${line}`);
      if (logs.length > 40) logs.shift();
    }
  };
  child.stdout?.on("data", remember);
  child.stderr?.on("data", remember);
}

async function waitForHttp(url, child) {
  const deadline = Date.now() + 180000;
  let last = "not started";
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`${url} exited ${child.exitCode}: ${last}`);
    try {
      const response = await fetch(url);
      if (response.ok) return;
      last = String(response.status);
    } catch (error) {
      last = error instanceof Error ? error.message : "fetch failed";
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`timed out waiting for ${url}: ${last}`);
}

async function api(url, path, payload, token) {
  const response = await fetch(`${url}${path}`, {
    method: payload === undefined ? "GET" : "POST",
    headers: {
      ...(payload === undefined ? {} : { "content-type": "application/json" }),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: payload === undefined ? undefined : JSON.stringify(payload),
  });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) throw new Error(`${path} ${response.status} ${body?.error ?? text}`);
  return body;
}

const work = await mkdtemp(join(tmpdir(), "cairn-graph-shot-"));
const store = join(work, "store.json");
const apiPort = 18081;
const webPort = 18082;
const apiUrl = `http://127.0.0.1:${apiPort}`;
const webUrl = `http://127.0.0.1:${webPort}`;
const server = spawn("cargo", ["run", "-q", "-p", "cairn-server", "--", "--store", store, "--bind", `127.0.0.1:${apiPort}`], {
  cwd: root,
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, CAIRN_URL: "", CAIRN_DEVICE_TOKEN: "", CAIRN_OUTBOX: "" },
});
listen(server, "api");
const web = spawn("./node_modules/.bin/next", ["dev", "--hostname", "127.0.0.1", "--port", String(webPort)], {
  cwd: join(root, "apps/web"),
  stdio: ["ignore", "pipe", "pipe"],
  env: { ...process.env, CAIRN_API_URL: apiUrl, CAIRN_URL: "", CAIRN_DEVICE_TOKEN: "", CAIRN_OUTBOX: "" },
});
listen(web, "web");

try {
  await waitForHttp(`${apiUrl}/cairn-memory/health`, server);
  const owner = await api(apiUrl, "/cairn-memory/auth/bootstrap", {
    email: "owner@example.test",
    password,
  });
  const personal = await api(apiUrl, "/cairn-memory/remember", {
    scope: "personal",
    kind: "fact",
    title: "Personal preference",
    summary: "Keep notes short.",
    reason: "Verified for the canvas.",
    statement: "Keep notes short.",
  }, owner.token);
  const personalFix = await api(apiUrl, "/cairn-memory/remember", {
    scope: "personal",
    kind: "gotcha",
    title: "Short note fix",
    summary: "Trim the summary before save.",
    reason: "Verified for the canvas.",
    symptom: "Long notes",
    cause: "No limit",
    fix: "Trim the summary",
  }, owner.token);
  const global = await api(apiUrl, "/cairn-memory/remember", {
    scope: "global",
    kind: "fact",
    title: "Global lesson",
    summary: "Redact secrets before save.",
    reason: "Verified for the canvas.",
    statement: "Redact secrets before save.",
  }, owner.token);
  const globalCause = await api(apiUrl, "/cairn-memory/remember", {
    scope: "global",
    kind: "rule",
    title: "Redaction rule",
    summary: "Scrub secrets before truncation.",
    reason: "Verified for the canvas.",
    rule: "Scrub first",
  }, owner.token);
  const project = await api(apiUrl, "/cairn-memory/projects/init", {
    repository: "cairn-demo",
    name: "Cairn Demo",
  }, owner.token);
  const decision = await api(apiUrl, "/cairn-memory/remember", {
    scope: "project",
    kind: "decision",
    title: "Cache decision",
    summary: "Cache project reads.",
    reason: "Verified for the canvas.",
    context: { project_id: project.project_id, repository: "cairn-demo" },
  }, owner.token);
  const follow = await api(apiUrl, "/cairn-memory/remember", {
    scope: "project",
    kind: "decision",
    title: "Cache invalidation",
    summary: "Drop the cache when the branch changes.",
    reason: "Verified for the canvas.",
    context: { project_id: project.project_id, repository: "cairn-demo" },
  }, owner.token);
  await api(apiUrl, "/cairn-memory/links", {
    sourceId: personalFix.memory_id,
    relation: "fixes",
    targetId: personal.memory_id,
  }, owner.token);
  await api(apiUrl, "/cairn-memory/links", {
    sourceId: globalCause.memory_id,
    relation: "causes",
    targetId: global.memory_id,
  }, owner.token);
  await api(apiUrl, "/cairn-memory/links", {
    sourceId: follow.memory_id,
    relation: "depends_on",
    targetId: decision.memory_id,
  }, owner.token);
  await waitForHttp(webUrl, web);
  await mkdir(out, { recursive: true });
  const { chromium } = loadPlaywright();
  const browser = await chromium.launch({ headless: true });
  const shots = [];
  for (const [theme, width] of [["light", 1440], ["dark", 1440], ["light", 390], ["dark", 390]]) {
    const page = await browser.newPage({ viewport: { width, height: width === 390 ? 844 : 900 }, colorScheme: theme });
    await page.addInitScript((choice) => localStorage.setItem("cairn-theme", choice), theme);
    await page.goto(webUrl);
    await page.getByLabel("Email").waitFor({ timeout: 60000 });
    await page.getByLabel("Email").fill("owner@example.test");
    await page.getByLabel("Password").fill(password);
    const button = page.getByRole("button", { name: /Create account|Sign in/ });
    await button.click();
    await page.waitForURL("**/projects");
    await page.goto(`${webUrl}/graph`);
    await page.getByTestId("memory-graph").waitFor({ timeout: 20000 });
    await page.waitForTimeout(1200);
    const file = join(out, `graph-${theme}-${width}.png`);
    await page.screenshot({ path: file, fullPage: true });
    shots.push(file);
    await page.close();
  }
  await browser.close();
  console.log(shots.join("\n"));
} catch (error) {
  console.error(logs.join("\n"));
  throw error;
} finally {
  server.kill("SIGTERM");
  web.kill("SIGTERM");
}
