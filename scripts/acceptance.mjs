import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parse } from "smol-toml";
import { validateTrace, requireBrowserEvidence } from "./acceptance-trace.mjs";

const args = process.argv.slice(2);
const stages = [...Array.from({ length: 9 }, (_, number) => `P${number}`), "P8B"];
const phase = args.find((arg) => stages.includes(arg)) ?? "P0";
const gate = args.includes("--gate");
if (args.some((arg) => arg !== phase && arg !== "--gate")) {
  throw new Error("Usage: npm run acceptance -- [P0..P8|P8B] [--gate]");
}
const read = (path) => readFileSync(path, "utf8");
const map = parse(read("docs/feature-map.toml")).case;
validateTrace({ map, stages, read });

const selected = map.filter((entry) => stages.indexOf(entry.phase) <= stages.indexOf(phase));
const external = [];
const cargo = [];
const browser = [];
const upstream = [];
for (const entry of selected) {
  for (const layer of entry.layers) {
    const test = entry.tests[layer];
    if (layer === "rust-integration") cargo.push([entry.id, test]);
    else if (layer === "playwright") browser.push([entry.id, test]);
    else if (layer === "agentnexus" || layer === "des-baseline") {
      if (process.env.MOM_AGENTNEXUS) upstream.push([entry.id, test]);
      else external.push(`${entry.id}/${layer}`);
    } else throw new Error(`Unknown executable layer: ${layer}`);
  }
}

function run(command, commandArgs, env = process.env) {
  const result = spawnSync(command, commandArgs, { env, encoding: "utf8", maxBuffer: 8 * 1024 * 1024 });
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.status !== 0) throw new Error(`${command} failed (${result.error ?? result.status ?? result.signal})`);
  return result.stdout;
}

if (!cargo.length || !browser.length) throw new Error("No integration or browser test selected");
const resultsDir = resolve("test-results");
mkdirSync(resultsDir, { recursive: true });
for (const file of ["industry-replay.json", "restart-recovery.json", "agentnexus-industries.json", "five-industry-catalog.json"]) {
  rmSync(join(resultsDir,file), { force: true });
}
for (const [id, test] of cargo) {
  const stdout = run("cargo", ["test", "--locked", "--test", "stdio", test, "--", "--exact"], {
    ...process.env, MOM_ACCEPTANCE_RESULTS: resultsDir,
  });
  if (!stdout.includes(`test ${test} ... ok`)) throw new Error(`Missing successful test for ${id}`);
}
const directory = mkdtempSync(join(tmpdir(), "mom-acceptance-"));
try {
  mkdirSync("test-results", { recursive: true });
  rmSync("test-results/browser.json", { force: true });
  const pattern = browser.map(([, test]) => test).join("|");
  run("node", ["node_modules/@playwright/test/cli.js", "test", "--grep", pattern], {
    ...process.env,
    MOM_TEST_DB: join(directory, "probe.sqlite3"),
  });
  const report = JSON.parse(read("test-results/browser.json"));
  requireBrowserEvidence(report, browser);
} finally {
  rmSync(directory, { recursive: true, force: true });
}
if (upstream.length) {
  const stdout = run("bash", ["scripts/check-agentnexus.sh", process.env.MOM_AGENTNEXUS], {
    ...process.env, MOM_ACCEPTANCE_PHASE: phase,
  });
  for (const [id, test] of upstream) {
    if (!stdout.includes(`test ${test} ... ok`)) throw new Error(`Missing successful upstream evidence for ${id}`);
  }
}
if (external.length) {
  console.error(`No automated evidence registered: ${external.join(", ")}`);
  if (gate) process.exitCode = 1;
}
