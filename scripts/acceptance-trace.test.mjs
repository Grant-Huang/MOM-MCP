import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parse } from "smol-toml";
import { validateTrace, requireBrowserEvidence } from "./acceptance-trace.mjs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const stages = [...Array.from({ length: 9 }, (_, number) => `P${number}`), "P8B", "P8C", "P8D", "P9A", "P9B", "P9C", "P9D"];
const map = parse(read("docs/feature-map.toml")).case;
const check = (map) => validateTrace({ map, stages, read });

test("the repository feature map traces all current acceptance phases", () => {
  assert.doesNotThrow(() => check(map));
});

for (const [label, edit, error] of [
  ["wrong spec reference", (entry) => { entry.spec = "acceptance/P8.md"; }, /Broken acceptance reference: P8B-02/],
  ["unknown feature", (entry) => { entry.features = ["F999"]; }, /Broken feature reference: P8B-02/],
  ["empty test layers", (entry) => { entry.layers = []; }, /Invalid test layers: P8B-02/],
  ["missing test mapping", (entry) => { entry.tests = {}; }, /Missing or unsupported test mapping: P8B-02/],
]) {
  test(`rejects ${label}`, () => {
    const edited = structuredClone(map);
    edit(edited.find((entry) => entry.id === "P8B-02"));
    assert.throws(() => check(edited), error);
  });
}

test("rejects a missing acceptance case", () => {
  assert.throws(() => check(map.filter((entry) => entry.id !== "P8B-02")), /Missing acceptance case: P8B-02/);
});

test("requires one matching browser spec with executed, passing evidence", () => {
  const title = "P8B-UI Public order discovery and readiness";
  const required = [["P8B-UI", title]];
  const passing = { title, ok: true, tests: [{ status: "expected", results: [{ status: "passed" }] }] };
  const report = (...specs) => ({ suites: [{ suites: [{ specs }] }] });
  assert.doesNotThrow(() => requireBrowserEvidence(report(passing), required));
  for (const bad of [
    report(),
    report(passing, passing),
    report({ ...passing, tests: [] }),
    report({ ...passing, tests: [{ status: "skipped", results: [{ status: "skipped" }] }] }),
    report({ ...passing, tests: [{ status: "unexpected", results: [{ status: "failed" }] }] }),
  ]) {
    assert.throws(() => requireBrowserEvidence(bad, required), /Missing successful browser evidence for P8B-UI/);
  }
});
