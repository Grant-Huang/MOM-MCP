import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export function classify(paths) {
  const changes = paths.map((path) => {
    if (/^\.github\/|^scripts\/|^AGENTS\.md$|^src\/(probe|scenario|simulation)\.rs$/.test(path)) {
      return { path, risk: 3, reason: "CI, control boundary or shared execution" };
    }
    if (/^(src|crates|fixtures|config|tests|web)\/|^(Cargo|package)[.-]|^docs\/(feature-map\.toml|FEATURES\.md|DESIGN\.md|governance\/|acceptance\/)/.test(path)) {
      return { path, risk: 2, reason: "Business behavior, shared contracts or acceptance evidence" };
    }
    if (/\.md$/.test(path)) {
      return { path, risk: 0, reason: "Documentation path; confirm the diff changes no obligations or behavior" };
    }
    return { path, risk: 2, reason: "Impact unknown; review scope and expand verification" };
  });
  const score = Math.max(0, ...changes.map(({ risk }) => risk));
  return { suggested_risk: `R${score}`, changes, requires_human_confirmation: true };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [base, head] = process.argv.slice(2);
  if (![base, head].every((sha) => /^[a-f0-9]{40}$/.test(sha ?? ""))) {
    throw new Error("Usage: node scripts/pr-risk.mjs <base SHA> <head SHA>");
  }
  // Treat renames as deletion plus addition, so moving a sensitive file retains its risk.
  const paths = execFileSync("git", ["diff", "--name-only", "--no-renames", "-z", `${base}...${head}`, "--"],
    { encoding: "utf8" }).split("\0").filter(Boolean);
  console.log(JSON.stringify({ base, head, ...classify(paths) }, null, 2));
}
