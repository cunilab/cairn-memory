import { spawn } from "node:child_process";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const password = "correct horse battery staple";
const reportPath = join(root, "docs/audits/2026-11-12-actions/action-regression.json");

function loadPlaywright() {
  const require = createRequire(import.meta.url);
  try {
    return require("playwright");
  } catch {
    throw new Error(
      "playwright is not on NODE_PATH. Point NODE_PATH at an existing install. Do not add it to this repo.",
    );
  }
}

function waitForExit(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((resolve) => child.once("exit", resolve));
}

function listen(child, label, lines) {
  const remember = (chunk) => {
    const text = String(chunk);
    lines.push(
      ...text
        .split("\n")
        .filter(Boolean)
        .slice(-5)
        .map((line) => `${label}: ${line}`),
    );
    if (lines.length > 40) {
      lines.splice(0, lines.length - 40);
    }
  };
  child.stdout?.on("data", remember);
  child.stderr?.on("data", remember);
}

async function waitForHttp(url, child) {
  const deadline = Date.now() + 120000;
  let last = "not started";
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`${url} exited ${child.exitCode}: ${last}`);
    }
    try {
      const response = await fetch(url);
      if (response.ok || response.status < 500) {
        return;
      }
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
  if (!response.ok) {
    throw new Error(`${path} ${response.status} ${body?.error ?? text}`);
  }
  return body;
}

async function decision(apiUrl, token, projectId, suffix, content) {
  const sessionId = `action-${suffix}`;
  await api(
    apiUrl,
    "/cairn-memory/session/start",
    { session_id: sessionId, context: { repository: "cairn-demo", commit: "abc1234" } },
    token,
  );
  await api(
    apiUrl,
    "/cairn-memory/observe",
    { event_id: `evt-${suffix}`, session_id: sessionId, content, role: "agent", source: "hook" },
    token,
  );
  const ended = await api(apiUrl, "/cairn-memory/session/end", { session_id: sessionId }, token);
  const promoted = await api(
    apiUrl,
    "/cairn-memory/promote",
    { project_id: projectId, memory_id: ended.memory_id, reason: suffix },
    token,
  );
  return promoted.memory_id;
}

function assert(condition, message, report) {
  report.checks.push({ message, ok: Boolean(condition) });
  if (!condition) {
    report.failures.push(message);
  }
}

// Decision actions live in a row menu: open it, then pick the item by name.
async function rowAction(page, row, name) {
  await row.getByRole("button", { name: /^Actions for / }).click();
  await page.getByRole("menuitem", { name }).click();
}

async function signIn(page, web, email) {
  await page.context().clearCookies();
  await page.goto(`${web}/`);
  await page.locator("#email").fill(email);
  await page.locator("#password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL("**/projects");
}

const report = {
  startedAt: new Date().toISOString(),
  checks: [],
  failures: [],
  counts: {},
};
const logs = [];
let browser;
const work = await mkdtemp(join(tmpdir(), "cairn-actions-"));
const store = join(work, "store.json");
const apiPort = 52710 + Math.floor(Math.random() * 1000);
const webPort = apiPort + 1;
const apiUrl = `http://127.0.0.1:${apiPort}`;
const web = `http://127.0.0.1:${webPort}`;
const apiChild = spawn(
  "cargo",
  ["run", "-q", "-p", "cairn-server", "--", "--store", store, "--bind", `127.0.0.1:${apiPort}`],
  { cwd: root, detached: true, stdio: ["ignore", "pipe", "pipe"] },
);
listen(apiChild, "api", logs);
const webChild = spawn(
  "./node_modules/.bin/next",
  ["dev", "--hostname", "127.0.0.1", "--port", String(webPort)],
  {
    cwd: join(root, "apps/web"),
    detached: true,
    env: { ...process.env, CAIRN_API_URL: apiUrl, NEXT_TELEMETRY_DISABLED: "1" },
    stdio: ["ignore", "pipe", "pipe"],
  },
);
listen(webChild, "web", logs);

try {
  await waitForHttp(`${apiUrl}/cairn-memory/health`, apiChild);
  await waitForHttp(`${web}/`, webChild);
  const owner = await api(apiUrl, "/cairn-memory/auth/bootstrap", {
    email: "owner@example.test",
    password,
  });
  const token = owner.token;
  await api(
    apiUrl,
    "/cairn-memory/auth/register",
    { email: "member@example.test", password },
    token,
  );
  const project = await api(
    apiUrl,
    "/cairn-memory/projects/init",
    { repository: "cairn-demo", name: "Cairn Demo" },
    token,
  );
  const projectId = project.project_id;
  // Membership is automatic: the member captures with the project id in the context.
  const memberLogin = await api(apiUrl, "/cairn-memory/auth/login", {
    email: "member@example.test",
    password,
  });
  const memberDevice = await api(
    apiUrl,
    "/cairn-memory/devices/register",
    { name: "member agent" },
    memberLogin.token,
  );
  await api(
    apiUrl,
    "/cairn-memory/observe",
    {
      eventId: "member-join",
      sessionId: "member-join",
      content: "joined by project file",
      role: "agent",
      source: "smoke",
      context: { repository: "cairn-demo", project_id: projectId },
    },
    memberDevice.token,
  );
  await api(
    apiUrl,
    "/cairn-memory/auth/register",
    { email: "reader@example.test", password },
    token,
  );
  const readerLogin = await api(apiUrl, "/cairn-memory/auth/login", {
    email: "reader@example.test",
    password,
  });
  await api(
    apiUrl,
    "/cairn-memory/projects/init",
    { repository: "cairn-demo", project_id: projectId },
    readerLogin.token,
  );
  const deleteId = await decision(
    apiUrl,
    token,
    projectId,
    "delete",
    "Disposable delete decision.",
  );
  const cancelId = await decision(
    apiUrl,
    token,
    projectId,
    "cancel",
    "Disposable cancel decision must remain.",
  );
  const refreshId = await decision(
    apiUrl,
    token,
    projectId,
    "refresh",
    "Disposable refresh decision is hidden before reload fails.",
  );
  const promoteText = "Full copied publication payload ".repeat(60) + "END OF FULL PREVIEW";
  await api(
    apiUrl,
    "/cairn-memory/remember",
    { content: promoteText, context: { repository: "cairn-demo", files: ["src/full-payload.rs"] } },
    token,
  );
  const memberText = "Member authored publication from private memory.";
  await api(
    apiUrl,
    "/cairn-memory/remember",
    { content: memberText, context: { repository: "cairn-demo" } },
    memberLogin.token,
  );
  const branchText = "Personal branch evidence reviewed before recording a merge.";
  await api(apiUrl, `/cairn-memory/projects/${projectId}/branches`, { name: "feature/ui-merge" }, token);
  await api(apiUrl, "/cairn-memory/remember", {
    content: branchText,
    context: { repository: "cairn-demo", branch: "feature/ui-merge" },
  }, token);
  report.fixture = { projectId, deleteId, cancelId, refreshId };

  const { chromium } = loadPlaywright();
  browser = await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const posts = [];
  page.on("request", (request) => {
    const path = request.url().split("/api/cairn/")[1] ?? "";
    if (
      request.method() === "POST" &&
      (path === "delete" || path === "promote" || path.endsWith("/ownership") || path.endsWith("/merge") || path.endsWith("/close"))
    )
      posts.push({ path, body: request.postDataJSON() });
  });
  await signIn(page, web, "owner@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  const cancelCard = page.getByRole("row").filter({ hasText: "must remain" });
  await cancelCard.waitFor();
  const before = posts.length;
  await rowAction(page, cancelCard, "Delete");
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  assert(
    (await dialog.innerText()).includes(cancelId),
    "confirmation identifies its decision",
    report,
  );
  assert(posts.length === before, "opening delete sends no POST", report);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  await rowAction(page, cancelCard, "Delete");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(posts.length === before, "Escape and Cancel send no POST", report);

  await page.route("**/api/cairn/delete", (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "database unavailable" }),
    }),
  );
  await rowAction(page, cancelCard, "Delete");
  await dialog.getByRole("button", { name: "Delete decision", exact: true }).click();
  await dialog.getByRole("alert").waitFor();
  assert(
    (await dialog.getByRole("alert").innerText()) === "database unavailable",
    "failed mutation keeps the server error visible",
    report,
  );
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(await cancelCard.isVisible(), "failed mutation keeps the row", report);
  await page.unroute("**/api/cairn/delete");

  const deleteCard = page.getByRole("row").filter({ hasText: "Disposable delete decision" });
  await rowAction(page, deleteCard, "Delete");
  await dialog.getByRole("button", { name: "Delete decision", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await deleteCard.waitFor({ state: "hidden" });
  assert(
    posts.filter((p) => p.path === "delete").length === 2,
    "failed delete and confirmed success each send one POST",
    report,
  );

  await page.route(`**/api/cairn/projects/${projectId}/memories`, (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "reload failed" }),
    }),
  );
  const refreshCard = page.getByRole("row").filter({ hasText: "reload fails" });
  await rowAction(page, refreshCard, "Delete");
  await dialog.getByRole("button", { name: "Delete decision", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.getByRole("alert").filter({ hasText: "do not repeat the action" }).waitFor();
  assert(
    (await refreshCard.count()) === 0,
    "successful deletion remains hidden after refresh failure",
    report,
  );
  assert(
    posts.filter((p) => p.path === "delete").length === 3,
    "refresh failure never repeats the mutation",
    report,
  );
  await page.unroute(`**/api/cairn/projects/${projectId}/memories`);
  await page.reload();

  const verifyRow = page.getByRole("row").filter({ hasText: "must remain" });
  await verifyRow.waitFor();
  await rowAction(page, verifyRow, "Verify");
  await dialog.getByLabel("Commit", { exact: true }).fill("verified-ui");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await verifyRow.getByText("verified-ui").waitFor();
  assert(true, "owner verifies a shared decision", report);
  await rowAction(page, verifyRow, "Correct");
  await dialog
    .getByLabel("Corrected decision")
    .fill("Disposable cancel decision must remain after correction.");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.getByRole("row").filter({ hasText: "must remain after correction" }).waitFor();
  assert(true, "owner correction stays in the common project list", report);

  await page.getByRole("tab", { name: "Memories", exact: true }).click();
  const memoryRow = page.getByRole("row").filter({ hasText: "END OF FULL PREVIEW" });
  await memoryRow.waitFor();
  await rowAction(page, memoryRow, "Promote to decision");
  await dialog.getByLabel("Reason").fill("Preview reviewed by every author");
  await dialog.getByRole("button", { name: "Promote", exact: true }).click();
  await dialog.getByRole("alert").waitFor();
  assert(
    posts.filter((p) => p.path === "promote" && !p.body.dryRun).length === 0,
    "web publication requires a preview",
    report,
  );
  await dialog.getByRole("button", { name: "Preview full payload" }).click();
  await dialog.locator("pre").filter({ hasText: "END OF FULL PREVIEW" }).waitFor();
  assert(
    (await dialog.locator("pre").innerText()).includes("src/full-payload.rs"),
    "preview includes complete content and metadata",
    report,
  );
  await dialog.getByRole("button", { name: "Promote", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.getByRole("tab", { name: "Decisions", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "END OF FULL PREVIEW" }).waitFor();
  await page.screenshot({ path: join(root, "target/project-model-ui.png"), fullPage: true });

  await page.getByRole("tab", { name: "Memories", exact: true }).click();
  await page.getByRole("button", { name: "Record merge", exact: true }).click();
  await dialog.getByRole("combobox").click();
  await page.getByRole("option", { name: branchText }).click();
  await dialog.getByLabel("Merge commit").fill("ui-merge-commit");
  await dialog.getByLabel("Reason", { exact: true }).fill("Reviewed branch evidence");
  await dialog.getByRole("button", { name: "Preview full payload" }).click();
  await dialog.locator("pre").filter({ hasText: "ui-merge-commit" }).waitFor();
  await dialog.getByRole("button", { name: "Record merge", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(
    (await api(apiUrl, `/cairn-memory/projects/${projectId}/branches`, undefined, token))
      .find((b) => b.name === "feature/ui-merge").status === "merged",
    "Record merge previews and publishes evidence while recording the caller's branch",
    report,
  );
  await api(apiUrl, `/cairn-memory/projects/${projectId}/branches`, { name: "feature/ui-close" }, token);
  await page.reload();
  await page.getByRole("tab", { name: "Memories", exact: true }).click();
  await page.getByRole("button", { name: "Close branch", exact: true }).click();
  await dialog.getByLabel("Reason", { exact: true }).fill("Personal branch no longer needed");
  await dialog.getByRole("button", { name: "Close branch", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(
    (await api(apiUrl, `/cairn-memory/projects/${projectId}/branches`, undefined, token))
      .find((b) => b.name === "feature/ui-close").status === "closed",
    "Close branch records the caller's branch status",
    report,
  );

  await signIn(page, web, "member@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  const ownerRow = page.getByRole("row").filter({ hasText: "must remain after correction" });
  await ownerRow.waitFor();
  assert(
    (await ownerRow.getByRole("button", { name: /^Actions for / }).count()) === 0,
    "non-author member reads an owner decision without edit actions",
    report,
  );
  await page.getByRole("tab", { name: "Memories", exact: true }).click();
  const ownMemory = page.getByRole("row").filter({ hasText: memberText });
  await ownMemory.waitFor();
  await rowAction(page, ownMemory, "Promote to decision");
  await dialog.getByLabel("Reason").fill("Member publication");
  await dialog.getByRole("button", { name: "Preview full payload" }).click();
  await dialog.locator("pre").waitFor();
  await dialog.getByRole("button", { name: "Promote", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.getByRole("tab", { name: "Decisions", exact: true }).click();
  const authored = page.getByRole("row").filter({ hasText: memberText });
  await authored.waitFor();
  assert(
    (await authored.getByRole("button", { name: /^Actions for / }).count()) === 1,
    "member author has actions for their own publication",
    report,
  );
  assert(
    (await ownerRow.getByRole("button", { name: /^Actions for / }).count()) === 0,
    "member authorship does not grant rights to other decisions",
    report,
  );

  await signIn(page, web, "owner@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  await page.getByRole("tab", { name: "Members", exact: true }).click();
  const memberRow = page.getByRole("row").filter({ hasText: "member@example.test" });
  await memberRow.getByRole("button", { name: "Transfer ownership" }).click();
  await dialog.getByRole("button", { name: "Transfer ownership", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  const roster = await api(apiUrl, `/cairn-memory/projects/${projectId}/members`, undefined, token);
  assert(
    roster[0].email === "member@example.test",
    "owner transfer changes governance and retains the old owner as member",
    report,
  );
  await page.goto(`${web}/accounts`);
  await page.getByRole("row").filter({ hasText: "owner@example.test" }).waitFor();
  await rowAction(
    page,
    page.getByRole("row").filter({ hasText: "owner@example.test" }),
    "Transfer project ownership",
  );
  await dialog.getByLabel("Project ID").fill(projectId);
  await dialog.getByRole("button", { name: "Transfer ownership", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(
    (await api(apiUrl, `/cairn-memory/projects/${projectId}/members`, undefined, token))[0]
      .email === "owner@example.test",
    "admin recovers ownership from Accounts",
    report,
  );
  const formDialog = dialog;
  // Accounts: new user dialog, row menu password reset, both with inline validation.
  await page.goto(`${web}/accounts`);
  await page.getByRole("button", { name: "New user" }).click();
  const userDialog = page.getByRole("dialog");
  await userDialog.getByLabel("Email").fill("ui-user@example.test");
  await userDialog.getByLabel("Password", { exact: true }).fill("short");
  await userDialog.getByLabel("Confirm password", { exact: true }).fill("short");
  await userDialog.getByRole("button", { name: "Create user" }).click();
  assert(
    (await userDialog.getByRole("alert").innerText()).includes("at least 12"),
    "a short password is rejected in the dialog",
    report,
  );
  await userDialog.getByLabel("Password", { exact: true }).fill(password);
  await userDialog.getByLabel("Confirm password", { exact: true }).fill(password);
  await userDialog.getByRole("button", { name: "Create user" }).click();
  await userDialog.waitFor({ state: "hidden" });
  const userRow = page.getByRole("row").filter({ hasText: "ui-user@example.test" });
  await userRow.waitFor();
  assert(
    (await userRow.innerText()).includes("User"),
    "new account is listed with the User role",
    report,
  );
  assert(
    (await page.getByRole("row").filter({ hasText: "owner@example.test" }).innerText()).includes(
      "Admin",
    ),
    "the bootstrap account is listed as Admin",
    report,
  );
  const resetPassword = "reset horse battery staple";
  await rowAction(page, userRow, "Reset password");
  await userDialog.getByLabel("New password", { exact: true }).fill(resetPassword);
  await userDialog.getByLabel("Confirm password", { exact: true }).fill("something else entirely");
  await userDialog.getByRole("button", { name: "Reset password" }).click();
  assert(
    (await userDialog.getByRole("alert").innerText()).includes("do not match"),
    "a mismatched reset password is rejected in the dialog",
    report,
  );
  await userDialog.getByLabel("Confirm password", { exact: true }).fill(resetPassword);
  await userDialog.getByRole("button", { name: "Reset password" }).click();
  await userDialog.waitFor({ state: "hidden" });
  await api(apiUrl, "/cairn-memory/auth/login", {
    email: "ui-user@example.test",
    password: resetPassword,
  });
  const oldStillWorks = await api(apiUrl, "/cairn-memory/auth/login", {
    email: "ui-user@example.test",
    password,
  }).then(
    () => true,
    () => false,
  );
  assert(!oldStillWorks, "reset replaces the old password and the new one signs in", report);

  // Profile: change password (a wrong current password stays in the dialog) and the theme switch.
  await api(
    apiUrl,
    "/cairn-memory/auth/register",
    { email: "ui-profile@example.test", password },
    token,
  );
  await signIn(page, web, "ui-profile@example.test");
  const accountMenu = async () => {
    await page.locator("[data-slot=sidebar-footer] button").first().click();
  };
  await accountMenu();
  await page.getByRole("menuitem", { name: "Change password" }).click();
  await userDialog.getByLabel("Current password", { exact: true }).fill("not the current password");
  await userDialog.getByLabel("New password", { exact: true }).fill("profile horse battery staple");
  await userDialog.getByLabel("Confirm new password").fill("profile horse battery staple");
  await userDialog.getByRole("button", { name: "Change password" }).click();
  assert(
    (await userDialog.getByRole("alert").innerText()).includes("current password is incorrect"),
    "a wrong current password shows an inline error",
    report,
  );
  await userDialog.getByLabel("Current password", { exact: true }).fill(password);
  await userDialog.getByRole("button", { name: "Change password" }).click();
  await userDialog.waitFor({ state: "hidden" });
  await api(apiUrl, "/cairn-memory/auth/login", {
    email: "ui-profile@example.test",
    password: "profile horse battery staple",
  });
  assert(true, "changing the password signs in with the new one", report);
  const isDark = () => page.evaluate(() => document.documentElement.classList.contains("dark"));
  await accountMenu();
  await page.getByRole("menuitem", { name: "Theme" }).click();
  await page.getByRole("menuitemradio", { name: "Dark" }).click();
  assert(await isDark(), "choosing Dark applies the dark class", report);
  await page.reload({ waitUntil: "domcontentloaded" });
  assert(await isDark(), "the theme survives a reload without a flash", report);
  await accountMenu();
  await page.getByRole("menuitem", { name: "Theme" }).click();
  await page.getByRole("menuitemradio", { name: "Light" }).click();
  assert(!(await isDark()), "choosing Light removes the dark class", report);

  await signIn(page, web, "reader@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  await page.getByRole("row").filter({ hasText: "must remain after correction" }).waitFor();
  assert(
    (await page.getByRole("button", { name: /^Actions for / }).count()) === 0,
    "read-only member has no decision edit actions",
    report,
  );
  assert(
    (await page.getByRole("tab", { name: "Memories", exact: true }).count()) === 1,
    "every member has their own Memories tab",
    report,
  );
  for (const removed of ["Share decision", "Revoke share", "Controls", "Reports"])
    assert(
      (await page.getByText(removed, { exact: true }).count()) === 0,
      `removed UI: ${removed}`,
      report,
    );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await page.getByRole("row").filter({ hasText: "must remain after correction" }).waitFor();
  assert(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    "project decisions fit a phone viewport without page overflow",
    report,
  );
  await page.screenshot({ path: join(root, "target/project-model-mobile.png"), fullPage: true });
  report.counts = {
    deletePosts: posts.filter((p) => p.path === "delete").length,
    previewPosts: posts.filter((p) => p.path === "promote" && p.body.dryRun).length,
    publicationPosts: posts.filter((p) => p.path === "promote" && !p.body.dryRun).length,
    mergePosts: posts.filter((p) => p.path.endsWith("/merge")).length,
    closePosts: posts.filter((p) => p.path.endsWith("/close")).length,
    ownershipTransfers: posts.filter((p) => p.path.endsWith("/ownership")).length,
  };
  assert(
    report.counts.deletePosts === 3 &&
      report.counts.previewPosts === 3 &&
      report.counts.publicationPosts === 2 &&
      report.counts.ownershipTransfers === 2 &&
      report.counts.mergePosts === 1 && report.counts.closePosts === 1,
    "project actions send exactly the expected requests",
    report,
  );
  await browser.close();
} catch (error) {
  report.failures.push(error instanceof Error ? (error.stack ?? error.message) : String(error));
} finally {
  await browser?.close();
  for (const child of [webChild, apiChild]) {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  await Promise.all([waitForExit(webChild), waitForExit(apiChild)]);
  await rm(work, { recursive: true, force: true });
  report.finishedAt = new Date().toISOString();
  report.logs = logs;
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(
    JSON.stringify({
      ok: report.failures.length === 0,
      failures: report.failures,
      counts: report.counts,
    }),
  );
  process.exit(report.failures.length === 0 ? 0 : 1);
}
