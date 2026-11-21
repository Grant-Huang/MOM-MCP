import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parse } from "smol-toml";

const args = process.argv.slice(2);
const phase = args.find((arg) => /^P[0-7]$/.test(arg)) ?? "P0";
const gate = args.includes("--gate");
if (args.some((arg) => arg !== phase && arg !== "--gate")) {
  throw new Error("Usage: npm run acceptance -- [P0..P7] [--gate]");
}
const read = (path) => readFileSync(path, "utf8");
const map = parse(read("docs/feature-map.toml")).case;
const features = [...read("docs/FEATURES.md").matchAll(/^\| (F\d+) \|/gm)].map((match) => match[1]);
const expected = Array.from({ length: 8 }, (_, number) => {
  const stage = `P${number}`;
  const cases = [...read(`docs/acceptance/${stage}.md`).matchAll(/^\| (P\d-\d+) \|/gm)];
  return [`${stage}-UI`, ...cases.map((match) => match[1])];
}).flat();
if (
  map.length !== expected.length ||
  new Set(map.map(({ id }) => id)).size !== expected.length ||
  map.some(({ id, phase, features: covered, spec }) =>
    !expected.includes(id) || !id.startsWith(`${phase}-`) ||
    !covered.length || covered.some((name) => !features.includes(name)) ||
    !read(`docs/${spec}`).includes(id)
  ) ||
  features.some((feature) => !map.some((entry) => entry.features.includes(feature)))
) {
  throw new Error("Feature map and acceptance specifications disagree");
}

const selected = map.filter((entry) => entry.phase <= phase);
const external = [];
const missing = [];
const cargo = [];
const browser = [];
const upstream = [];
for (const entry of selected) {
  for (const layer of entry.layers) {
    const test = entry.tests?.[layer];
    if (!test) missing.push(`${entry.id}/${layer}`);
    else if (layer === "rust-integration") cargo.push([entry.id, test]);
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
  function specs(suite) {
    return [...(suite.specs ?? []), ...(suite.suites ?? []).flatMap(specs)];
  }
  const results = report.suites.flatMap(specs);
  for (const [id, title] of browser) {
    const matches = results.filter((spec) => spec.title === title);
    if (matches.length !== 1 || !matches[0].ok ||
        matches[0].tests.some((test) => test.status !== "expected" || test.results.at(-1)?.status !== "passed")) {
      throw new Error(`Missing successful browser evidence for ${id}`);
    }
  }
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
if (missing.length) {
  console.error(`Missing project tests: ${missing.join(", ")}`);
  process.exitCode = 1;
}
if (external.length) {
  console.error(`No automated evidence registered: ${external.join(", ")}`);
  if (gate) process.exitCode = 1;
}
