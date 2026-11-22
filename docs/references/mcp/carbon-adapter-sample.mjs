// Run with Node 24 and the pinned Carbon checkout as the first argument.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { stripTypeScriptTypes } from "node:module";

const root = process.argv[2];
assert.ok(root, "Pass the Carbon checkout path");
const read = (path) => readFileSync(join(root, path), "utf8");
const production = "apps/erp/app/modules/production";
const manifest = JSON.parse(read("apps/erp/app/routes/api+/mcp+/lib/tool-manifest.digest.json"));
const entry = Object.values(manifest).flat().find(({ name }) => name === "production_getJob");
assert.equal(entry?.classification, "READ");
assert.equal(entry.permission, "production:view");
assert.equal(entry.paginates, false);

const registration = read(`${production}/production.server.ts`)
  .match(/export const getJob = defineServiceFn\(\{[\s\S]*?^\}\);/m)?.[0];
assert.match(registration, /id: z\.string\(\)/);
assert.match(registration, /getJob\(ctx\.asCaller\.supabase, input\.id/);

const source = read(`${production}/production.service.ts`)
  .match(/^export async function getJob\(.*?^\}/ms)?.[0];
assert.ok(source, "Carbon getJob service not found");
const code = stripTypeScriptTypes(source);
const { getJob } = await import(`data:text/javascript,${encodeURIComponent(code)}`);

const row = {
  id: "source-record-1",
  jobId: "JOB-SAMPLE-1",
  companyId: "sample-company",
  status: "Draft",
};
const results = [
  [row.id, { data: row, error: null }],
  ["unseen-id", { data: null, error: { code: "PGRST116" } }],
];
for (const [id, result] of results) {
  const callerClient = {
    from(name) {
      assert.equal(name, "jobs");
      return {
        select(columns) {
          assert.equal(columns, "*");
          return {
            eq(column, value) {
              assert.equal(column, "id");
              assert.equal(value, id);
              return { single: async () => result };
            },
          };
        },
      };
    },
  };
  assert.deepEqual(await getJob(callerClient, id), result);
}
console.log("Carbon production_getJob: read registration and caller client query verified with two fixture outcomes.");
