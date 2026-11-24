import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { formatWhyNone, hasSaves, renderOutcomeStatus, summaryLine } from "./session-summary.ts";

test("the sessions table names the time-window count", () => {
  const sheet = readFileSync(new URL("../_components/project-sessions.tsx", import.meta.url), "utf8");
  assert.match(sheet, /New records in this time window/);
  assert.match(sheet, /new records during each session/);
});

test("renders_summary_line", () => {
  assert.equal(summaryLine(0, 0), "0 sessions, 0 with saves");
  assert.equal(summaryLine(1, 0), "1 sessions, 0 with saves");
  assert.equal(summaryLine(5, 2), "5 sessions, 2 with saves");
});

test("only notes and agent saves count toward the summary", () => {
  const session = { notes_written: 0, agent_saves: 0, digest_written: true, handoff_outcome: "written" };
  assert.equal(hasSaves(session), false);
  assert.equal(hasSaves({ ...session, notes_written: 1 }), true);
  assert.equal(hasSaves({ ...session, agent_saves: 1 }), true);
});

test("renderOutcomeStatus handles open and before 0.1.9", () => {
  // before 0.1.9: distilled_at is null
  assert.equal(renderOutcomeStatus({ distilled_at: null, ended_at: null }), "before 0.1.9");
  assert.equal(renderOutcomeStatus({ distilled_at: null, ended_at: 12345 }), "before 0.1.9");

  // open: distilled_at is set but ended_at is null
  assert.equal(renderOutcomeStatus({ distilled_at: 10000, ended_at: null }), "open");

  // ended and distilled: null
  assert.equal(renderOutcomeStatus({ distilled_at: 10000, ended_at: 10050 }), null);
});

test("formatWhyNone formats fixed why-none codes", () => {
  assert.equal(formatWhyNone(["no_edits", "no_typed_lines"]), "no edits, no decision lines");
  assert.equal(formatWhyNone(["no_test_runs", "no_fixed_failures"]), "no test runs, no fixed failures");
  assert.equal(formatWhyNone(["all_duplicates", "automated"]), "all duplicates, automated");
  assert.equal(formatWhyNone(["all_forgotten"]), "all matched forgotten notes");
});
