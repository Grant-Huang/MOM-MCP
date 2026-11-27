import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export function missingReviewContent(body) {
  const visible = (body ?? "").replace(/<!--[\s\S]*?(?:-->|$)/g, "");
  const sections = new Map();
  let section;
  for (const line of visible.split(/\r?\n/)) {
    const heading = line.match(/^##\s+(.+?)\s*$/);
    if (heading) {
      section = heading[1];
      sections.set(section, []);
    } else if (section) {
      sections.get(section).push(line.trim());
    }
  }
  const missing = [];
  const summary = sections.get("给审查人") ?? [];
  const fields = [
    ["做了什么", "用一句话写使用者可见的结果或系统行为。"],
    ["为什么", "用一句话写这次解决的问题。"],
    ["需要你做什么", "批准、拍板某个选择或无。"],
  ];
  for (const [label, placeholder] of fields) {
    const row = new RegExp(`^(?:[-*]\\s+)?\\*\\*${label}[：:]?\\*\\*[：:]?\\s*(.*)$`);
    const value = summary.map((line) => line.match(row)?.[1].trim()).find(Boolean);
    if (!value || value === placeholder) missing.push(`给审查人 / ${label}`);
  }
  const details = sections.get("技术细节") ?? [];
  const placeholders = new Set([
    "需求依据、设计一致性、影响范围与非目标。",
    "风险等级、判定理由、回滚方法及所需评审。",
    "验证命令、结果、跳过或未验证的项目。",
  ]);
  if (!details.some((line) => {
    const value = line.replace(/^[-*]\s+/, "");
    return value && !placeholders.has(value) && !value.startsWith("术语与编号首次出现");
  })) missing.push("技术细节");
  return missing;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, "utf8"));
  const missing = missingReviewContent(event.pull_request.body);
  if (missing.length) {
    console.error(`请用本次改动的实际内容填写：${missing.join("、")}`);
    process.exitCode = 1;
  }
}
