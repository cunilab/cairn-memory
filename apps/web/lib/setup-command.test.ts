import assert from "node:assert/strict";
import { test } from "node:test";
import { setupCommand } from "./setup-command.ts";

test("the setup command names the server's public url and the agent", () => {
  assert.equal(setupCommand("https://cairn.example.com", "claude"), "cairn setup https://cairn.example.com --agent claude");
  assert.equal(setupCommand("https://cairn.example.com/", "codex"), "cairn setup https://cairn.example.com --agent codex");
});

test("without a public url the command keeps a placeholder", () => {
  assert.equal(setupCommand(null, "opencode"), "cairn setup <server-url> --agent opencode");
  assert.equal(setupCommand("  ", "claude"), "cairn setup <server-url> --agent claude");
});
