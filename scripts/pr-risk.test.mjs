import assert from "node:assert/strict";
import test from "node:test";
import { classify } from "./pr-risk.mjs";

test("ordinary documentation suggests R0 and still needs confirmation", () => {
  const report = classify(["docs/IMPLEMENTATION_PLAN.md"]);
  assert.equal(report.suggested_risk, "R0");
  assert.equal(report.requires_human_confirmation, true);
});

test("acceptance and unknown paths receive wider review", () => {
  assert.equal(classify(["docs/acceptance/P8B.md"]).suggested_risk, "R2");
  assert.equal(classify(["unexpected.data"]).suggested_risk, "R2");
});

test("a mixed change retains the highest risk and lists every path", () => {
  const paths = ["docs/IMPLEMENTATION_PLAN.md", "src/mes.rs", ".github/workflows/acceptance.yml"];
  const report = classify(paths);
  assert.equal(report.suggested_risk, "R3");
  assert.deepEqual(report.changes.map(({ path }) => path), paths);
});
