import assert from "node:assert/strict";
import test from "node:test";
import { ALLOWED, upstreamPath } from "./proxy.ts";
import { MAX_INSTRUCTION_BYTES, byteCounter, byteLength, changedFields, overLimit, sampleStart, sampleStop } from "./agent-instructions.ts";

test("bytes are counted as UTF-8 after trimming, as the server does", () => {
  assert.equal(byteLength("  abc \n"), 3);
  assert.equal(byteLength("é".repeat(300)), 600);
  assert.equal(overLimit("é".repeat(300)), false);
  assert.equal(overLimit("é".repeat(301)), true);
  assert.equal(byteCounter("hello"), "5 / 600 bytes");
  assert.equal(byteCounter("é".repeat(301)), "602 / 600 bytes (2 over)");
  assert.equal(MAX_INSTRUCTION_BYTES, 600);
});

test("the preview shows what the agent receives", () => {
  assert.equal(sampleStart("  Use Cairn.  "), "Cairn session brief …\nUse Cairn.");
  assert.equal(sampleStop("Edited {files} files; {files} total."), "Edited 3 files; 3 total.");
  assert.equal(sampleStop("Edited {files}", 12), "Edited 12");
});

test("only changed fields are sent, and clearing sends an empty text", () => {
  const saved = { session_instruction: "Old line.", stop_nudge: null };
  assert.deepEqual(changedFields({ session_instruction: "Old line. ", stop_nudge: "" }, saved), {});
  assert.deepEqual(changedFields({ session_instruction: "New.", stop_nudge: "" }, saved), { session_instruction: "New." });
  assert.deepEqual(changedFields({ session_instruction: "", stop_nudge: "Save {files}" }, saved), {
    session_instruction: "",
    stop_nudge: "Save {files}",
  });
});

test("the console may reach the agent instructions route", () => {
  assert.ok((ALLOWED as readonly string[]).includes("agent-instructions"));
  assert.equal(upstreamPath(["agent-instructions"]), "agent-instructions");
  assert.equal(upstreamPath(["projects", "proj_1", "agent-instructions"]), "projects/proj_1/agent-instructions");
});
