import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { classify } from "./pr-risk.mjs";

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8" }).trimEnd();
}

export function audit(mode, head, previous = "", now = new Date()) {
  if (!["daily", "weekly"].includes(mode) ||
      !/^[a-f0-9]{40}$/.test(head ?? "") ||
      (previous && (!/^[a-f0-9]{40}$/.test(previous) || mode !== "daily"))) {
    throw new Error("Usage: node scripts/periodic-audit.mjs <daily|weekly> <head SHA> [previous successful Daily SHA]");
  }
  const weekStart = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000).toISOString();
  const history = git("rev-list", "--first-parent", head).split("\n");
  const root = history.at(-1);
  const cutoff = mode === "weekly"
    ? git("rev-list", "--first-parent", "-1", `--before=${weekStart}`, head)
    : "";
  const base = previous || cutoff || root;
  if (!history.includes(base)) {
    throw new Error("Daily baseline is outside the audited first-parent history; verify the history before retrying");
  }

  const paths = execFileSync("git", ["diff", "--name-only", "--no-renames", "-z", base, head, "--"],
    { encoding: "utf8" }).split("\0").filter(Boolean);
  const commits = history.slice(0, history.indexOf(base));
  return {
    mode, base_sha: base, head_sha: head, analyzed_at: now.toISOString(),
    baseline_source: previous ? "last_successful_daily" : cutoff ? "weekly_cutoff" : "repository_root",
    commits,
    ...classify(paths),
    ...(mode === "weekly" ? {
      window_start: weekStart,
      review_topics: [
        "Review contract and acceptance mapping changes for drift",
        "Correlate failed checks and review findings before proposing recurring-root-cause fixes",
        "Review guardrail cost and false positives before revising or retiring checks",
      ],
      unknowns: ["Independent review findings and historical CI trends require evidence outside this report"],
    } : {}),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  console.log(JSON.stringify(audit(...process.argv.slice(2)), null, 2));
}
