import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

test("daily catch-up, weekly cutoff, and invalid baseline", () => {
  const cwd = mkdtempSync(join(tmpdir(), "mom-audit-test-"));
  const script = new URL("./periodic-audit.mjs", import.meta.url).pathname;
  const git = (...args) => execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  function commit(path, time) {
    writeFileSync(join(cwd, path), time);
    git("add", "--", path);
    execFileSync("git", ["-c", "user.name=Audit test", "-c", "user.email=audit@example.invalid",
      "commit", "-qm", path], { cwd, env: { ...process.env, GIT_AUTHOR_DATE: time, GIT_COMMITTER_DATE: time } });
    return git("rev-parse", "HEAD");
  }
  function report(mode, head, ...args) {
    const clock = `import { audit } from ${JSON.stringify(new URL("./periodic-audit.mjs", import.meta.url).href)};
      console.log(JSON.stringify(audit(...process.argv.slice(1), new Date("2026-11-22T08:00:00Z"))));`;
    if (mode === "weekly") {
      return JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", clock,
        mode, head, ""], { cwd }));
    }
    return JSON.parse(execFileSync(process.execPath, [script, mode, head, ...args], { cwd }));
  }

  try {
    git("init", "-q");
    const root = commit("README.md", "2026-11-01T08:00:00Z");
    const baseline = commit("old.md", "2026-11-13T08:00:00Z");
    const missed = commit("missed.md", "2026-11-16T08:00:00Z");
    const head = commit("head.md", "2026-11-21T08:00:00Z");
    const daily = report("daily", head, baseline);
    assert.equal(daily.baseline_source, "last_successful_daily");
    assert.deepEqual(daily.commits, [head, missed]);
    assert.deepEqual(daily.changes.map(({ path }) => path), ["head.md", "missed.md"]);
    assert.equal(report("daily", head).base_sha, root);
    assert.deepEqual(report("daily", head, head).commits, []);
    assert.equal(report("weekly", head).base_sha, baseline);
    assert.throws(() => execFileSync(process.execPath, [script, "daily", baseline, head],
      { cwd, stdio: "pipe" }), /outside the audited first-parent history/);
    assert.equal(report("daily", head, baseline).base_sha, baseline);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
