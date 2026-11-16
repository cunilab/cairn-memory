import { spawn } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
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
  return new Promise((resolve) => {
    child.on("exit", (code) => resolve(code));
  });
}

function listen(child, label, lines) {
  const remember = (chunk) => {
    const text = String(chunk);
    lines.push(...text.split("\n").filter(Boolean).slice(-5).map((line) => `${label}: ${line}`));
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
  await api(
    apiUrl,
    `/cairn-memory/projects/${projectId}/shares`,
    { memory_id: promoted.memory_id, email: "member@example.test" },
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
const work = await mkdtemp(join(tmpdir(), "cairn-actions-"));
const store = join(work, "store.json");
const apiPort = 52710 + Math.floor(Math.random() * 1000);
const webPort = apiPort + 1;
const apiUrl = `http://127.0.0.1:${apiPort}`;
const web = `http://127.0.0.1:${webPort}`;
const apiChild = spawn(
  "cargo",
  ["run", "-q", "-p", "cairn-server", "--", "--store", store, "--bind", `127.0.0.1:${apiPort}`],
  { cwd: root, stdio: ["ignore", "pipe", "pipe"] },
);
listen(apiChild, "api", logs);
const webChild = spawn(
  "./node_modules/.bin/next",
  ["dev", "--hostname", "127.0.0.1", "--port", String(webPort)],
  {
    cwd: join(root, "apps/web"),
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
  const deleteId = await decision(
    apiUrl,
    token,
    projectId,
    "delete",
    "Disposable delete decision.",
  );
  const revokeId = await decision(
    apiUrl,
    token,
    projectId,
    "revoke",
    "Disposable revoke decision stays with the owner.",
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
  const promoteText = "Disposable memory promoted from the Memories tab.";
  await api(
    apiUrl,
    "/cairn-memory/remember",
    { content: promoteText, kind: "decision", concepts: [], context: { repository: "cairn-demo" } },
    token,
  );
  report.fixture = { projectId, deleteId, revokeId, cancelId, refreshId };

  const { chromium } = loadPlaywright();
  const browser = await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const posts = [];
  page.on("request", (request) => {
    if (request.method() !== "POST") {
      return;
    }
    const path = request.url().split("/api/cairn/")[1] ?? "";
    if (path === "delete" || path.includes("/revoke") || path.endsWith("/shares")) {
      posts.push(`${request.method()} ${path}`);
    }
  });
  await signIn(page, web, "owner@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  await page.getByText("must remain").waitFor();
  const before = posts.length;
  const cancelCard = page.getByRole("row").filter({ hasText: "must remain" });
  await rowAction(page, cancelCard, "Delete memory");
  const dialog = page.getByRole("alertdialog");
  await dialog.waitFor();
  assert(posts.length === before, "opening delete sends no POST", report);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  await rowAction(page, cancelCard, "Delete memory");
  await dialog.waitFor();
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await dialog.waitFor({ state: "hidden" });
  assert(posts.length === before, "Escape and Cancel send no POST", report);

  await page.route("**/api/cairn/delete", async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "database unavailable" }),
    });
  });
  await rowAction(page, cancelCard, "Delete memory");
  await dialog.waitFor();
  const failedTarget = await dialog.innerText();
  await dialog.getByRole("button", { name: "Delete memory" }).click();
  await dialog.getByRole("alert").waitFor();
  assert(
    (await dialog.getByRole("alert").innerText()) === "database unavailable",
    "mutation failure shows the server error",
    report,
  );
  assert(failedTarget.includes(cancelId), "failure dialog keeps the same decision", report);
  assert((await page.getByText("must remain").count()) === 1, "failed delete keeps the row", report);
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await page.unroute("**/api/cairn/delete");

  let held;
  let releaseHold;
  await page.route("**/api/cairn/projects/**", async (route) => {
    if (route.request().method() === "GET" && held) {
      await held;
    }
    await route.continue();
  });
  held = new Promise((resolve) => {
    releaseHold = resolve;
  });
  const deleteCard = page.getByRole("row").filter({ hasText: "Disposable delete decision" });
  await rowAction(page, deleteCard, "Delete memory");
  await dialog.waitFor();
  const confirm = dialog.getByRole("button", { name: "Delete memory" });
  const pendingClick = confirm.click();
  const working = dialog.getByRole("button", { name: "Working…" });
  await working.waitFor();
  assert(await working.isDisabled(), "confirm is disabled while the post runs", report);
  await working.click({ force: true }).catch(() => {});
  await page.getByText("Disposable delete decision").waitFor({ state: "hidden" });
  const postsWhileReload = posts.filter((item) => item.startsWith("POST delete")).length;
  assert(postsWhileReload === 2, "failed delete plus one success POST while reload is held", report);
  releaseHold();
  await pendingClick;
  await dialog.waitFor({ state: "hidden" });
  await page.unroute("**/api/cairn/projects/**");
  assert(
    (await page.getByText("Disposable delete decision").count()) === 0,
    "deleted decision stays gone after reload",
    report,
  );

  await page.route(`**/api/cairn/projects/${projectId}/memories`, async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ error: "reload failed" }),
      });
      return;
    }
    await route.continue();
  });
  const refreshCard = page.getByRole("row").filter({ hasText: "reload fails" });
  await rowAction(page, refreshCard, "Delete memory");
  await dialog.waitFor();
  const refreshPostsBefore = posts.filter((item) => item.startsWith("POST delete")).length;
  await dialog.getByRole("button", { name: "Delete memory" }).click();
  await page.getByText("Do not delete it again.").waitFor();
  assert(
    (await page.getByText("reload fails").count()) === 0,
    "failed refresh leaves the hidden decision off the page",
    report,
  );
  assert(
    posts.filter((item) => item.startsWith("POST delete")).length === refreshPostsBefore + 1,
    "failed refresh does not send a second delete",
    report,
  );
  await page.unroute(`**/api/cairn/projects/${projectId}/memories`);

  await page.locator("#member-email").fill("member@example.test");
  const revokeCard = page.getByRole("row").filter({ hasText: "Disposable revoke decision stays" });
  await rowAction(page, revokeCard, "Revoke share");
  await dialog.waitFor();
  assert(
    (await dialog.innerText()).includes("Share this decision again to show it to this member."),
    "revoke copy says sharing again restores access",
    report,
  );
  assert(
    !(await dialog.innerText()).includes("cannot undo the revoke"),
    "revoke copy does not deny the re-share",
    report,
  );
  const revokePostsBefore = posts.filter((item) => item.includes("/revoke")).length;
  await dialog.getByRole("button", { name: "Revoke share" }).click();
  await dialog.waitFor({ state: "hidden" });
  const revokePostsAfter = posts.filter((item) => item.includes("/revoke")).length;
  assert(revokePostsAfter === revokePostsBefore + 1, "confirming revoke sends one POST", report);
  assert(
    (await page.getByText("Disposable revoke decision stays").count()) === 1,
    "owner row remains after revoke",
    report,
  );
  await page.locator("#member-email").fill("member@example.test");
  const restoredCard = page.getByRole("row").filter({ hasText: "Disposable revoke decision stays" });
  const shareResponse = page.waitForResponse(
    (response) => response.url().includes("/shares") && response.request().method() === "POST",
  );
  await rowAction(page, restoredCard, "Share decision");
  const shared = await shareResponse;
  assert(shared.ok(), "sharing the revoked decision again is accepted", report);

  // Decisions row menu: verify against a commit. The commit field is required.
  await page.goto(`${web}/projects/${projectId}`);
  const verifyRow = page.getByRole("row").filter({ hasText: "must remain" });
  await verifyRow.waitFor();
  await rowAction(page, verifyRow, "Verify against commit");
  const formDialog = page.getByRole("dialog");
  await formDialog.waitFor();
  await formDialog.getByRole("button", { name: "Verify decision" }).click();
  assert(await formDialog.isVisible(), "verify without a commit keeps the dialog open", report);
  await formDialog.getByLabel("Commit").fill("abc1234");
  await formDialog.getByRole("button", { name: "Verify decision" }).click();
  await formDialog.waitFor({ state: "hidden" });
  await verifyRow.getByText("abc1234").waitFor();
  assert(true, "verify dialog records the commit on the decision row", report);

  // Memories tab: promote a personal memory from its row menu.
  await page.getByRole("tab", { name: "Memories" }).click();
  const memoryRow = page.getByRole("row").filter({ hasText: promoteText });
  await memoryRow.waitFor();
  await rowAction(page, memoryRow, "Promote to decision");
  await formDialog.waitFor();
  await formDialog.getByLabel("Reason").fill("Promoted from the console");
  await formDialog.getByRole("button", { name: "Promote", exact: true }).click();
  await formDialog.waitFor({ state: "hidden" });
  await page.getByRole("tab", { name: /^Decisions/ }).click();
  await page.getByRole("tabpanel").getByText(promoteText).waitFor();
  assert(true, "promoting from the Memories tab adds a decision", report);

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
  assert((await userRow.innerText()).includes("User"), "new account is listed with the User role", report);
  assert(
    (await page.getByRole("row").filter({ hasText: "owner@example.test" }).innerText()).includes("Admin"),
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
  await api(apiUrl, "/cairn-memory/auth/login", { email: "ui-user@example.test", password: resetPassword });
  const oldStillWorks = await api(apiUrl, "/cairn-memory/auth/login", {
    email: "ui-user@example.test",
    password,
  }).then(
    () => true,
    () => false,
  );
  assert(!oldStillWorks, "reset replaces the old password and the new one signs in", report);

  // Profile: change password (a wrong current password stays in the dialog) and the theme switch.
  await api(apiUrl, "/cairn-memory/auth/register", { email: "ui-profile@example.test", password }, token);
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

  await signIn(page, web, "member@example.test");
  await page.goto(`${web}/projects/${projectId}`);
  await page.getByText("must remain").waitFor();
  assert(
    (await page.getByText("Disposable revoke decision stays").count()) === 1,
    "sharing again restores the member read",
    report,
  );
  assert(
    (await page.getByText("Disposable delete decision").count()) === 0,
    "member does not see the deleted decision",
    report,
  );
  assert(
    (await page.getByRole("button", { name: /^Actions for / }).count()) === 0 &&
      (await page.getByRole("menuitem", { name: "Delete memory" }).count()) === 0,
    "member has no row actions or delete control",
    report,
  );

  report.counts = {
    deletePosts: posts.filter((item) => item.startsWith("POST delete")).length,
    revokePosts: posts.filter((item) => item.includes("/revoke")).length,
    sharePosts: posts.filter((item) => item.endsWith("/shares")).length,
  };
  assert(
    report.counts.deletePosts === 3,
    "delete posts are the failed one, the slow success, and the hidden refresh failure",
    report,
  );
  assert(report.counts.revokePosts === 1, "revoke posts are exactly one", report);
  await browser.close();
} catch (error) {
  report.failures.push(error instanceof Error ? error.stack ?? error.message : String(error));
} finally {
  webChild.kill("SIGTERM");
  apiChild.kill("SIGTERM");
  await Promise.all([waitForExit(webChild), waitForExit(apiChild)]);
  report.finishedAt = new Date().toISOString();
  report.logs = logs;
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ ok: report.failures.length === 0, failures: report.failures, counts: report.counts }));
  process.exit(report.failures.length === 0 ? 0 : 1);
}
