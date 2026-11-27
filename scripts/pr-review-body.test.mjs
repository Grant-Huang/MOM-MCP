import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { missingReviewContent } from "./pr-review-body.mjs";

test("empty bodies and an unfilled current template fail", () => {
  assert.equal(missingReviewContent(null).length, 4);
  const template = readFileSync(new URL("../.github/pull_request_template.md", import.meta.url), "utf8");
  assert.equal(missingReviewContent(template).length, 4);
});

test("legacy visible template prompts do not count as a description", () => {
  assert.equal(missingReviewContent(`## 给审查人
- **做了什么**　用一句话写使用者可见的结果或系统行为。
- **为什么**　用一句话写这次解决的问题。
- **需要你做什么**　批准、拍板某个选择或无。
## 技术细节
- 验证命令、结果、跳过或未验证的项目。`).length, 4);
});

test("a filled summary and concrete evidence pass", () => {
  assert.deepEqual(missingReviewContent(`## 给审查人
- **做了什么**　验证站点同号工单的读取隔离。
- **为什么：** 跨站点可能使用相同编号。
- **需要你做什么**：核对隔离范围。
## 需要你决定
无。
## 技术细节
- 通过同号工单的六种来源绑定测试。`), []);
});
