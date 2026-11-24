import assert from "node:assert/strict";
import { test } from "node:test";
import { buttonsFor } from "./fixes-buttons.ts";
import { readFileSync } from "node:fs";

test("buttons follow fix type and status; Undo is never offered", () => {
  const types = ["server", "computer", "agent", "report"] as const;
  const statuses = ["pending", "applying", "queued", "waiting", "done", "failed", "resolved", "ignored", "expired"] as const;
  for (const fix_type of types) for (const status of statuses) {
    const buttons = buttonsFor({ fix_type, status });
    let expected: string[] = [];
    if (status === "pending") expected = fix_type === "agent" ? ["Copy prompt", "Mark done", "Ignore"] : fix_type === "report" ? ["Ignore"] : ["Apply", "Ignore"];
    else if (status === "failed") expected = ["Apply", "Ignore"];
    else if (status === "waiting" && fix_type === "agent") expected = ["Copy prompt", "Mark done", "Ignore"];
    else if (status === "queued" && fix_type === "computer") expected = ["Ignore"];
    assert.deepEqual(buttons, expected, `${fix_type}/${status}`);
    assert.ok(!buttons.includes("Undo"));
  }
});

test("computer guidance names the device and commands", () => {
  const sheet = readFileSync(new URL("../_components/fixes.tsx", import.meta.url), "utf8");
  assert.match(sheet, /Runs on .* the next time someone there runs cairn doctor, cairn init or cairn fixes apply\./);
});

test("F9 Apply requires confirmation", () => {
  const sheet = readFileSync(new URL("../_components/fixes.tsx", import.meta.url), "utf8");
  assert.match(sheet, /<AlertDialog/);
  assert.match(sheet, /<AlertDialogAction[^>]*onClick=\{\(\) => void action\("Apply"\)\}/);
});
