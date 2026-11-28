import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { textSizeHint } from "./memory-text.ts";

describe("textSizeHint", () => {
  it("counts characters up to the suggested size plainly", () => {
    assert.deepEqual(textSizeHint("title", "é".repeat(80)), { label: "80/80", long: false });
  });

  it("notes a longer text softly, up to the maximum", () => {
    assert.deepEqual(textSizeHint("summary", "s".repeat(264)), {
      label: "264/240 suggested, 1000 max",
      long: true,
    });
  });
});
