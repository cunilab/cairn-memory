import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { pinFitText } from "./pin-summary.ts";

describe("pinFitText", () => {
  it("hides fit summary when no active pins exist", () => {
    assert.deepEqual(pinFitText({ shown: 0, total: 0, not_shown: 0 }), {
      fit: null,
      overflow: null,
    });
  });

  it("reports how many pins fit without overflow when all fit", () => {
    assert.deepEqual(pinFitText({ shown: 4, total: 4, not_shown: 0 }), {
      fit: "4 of 4 pins fit in the brief",
      overflow: null,
    });
  });

  it("reports overflow count when pins exceed L0 room", () => {
    assert.deepEqual(pinFitText({ shown: 4, total: 7, not_shown: 3 }), {
      fit: "4 of 7 pins fit in the brief",
      overflow: "3 more pinned not shown",
    });
  });
});
