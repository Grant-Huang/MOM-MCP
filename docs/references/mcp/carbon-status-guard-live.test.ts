// Copy to apps/erp/test in the pinned Carbon checkout. Uses an isolated stack.
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createClient } from "@supabase/supabase-js";
import { expect, it, vi } from "vitest";

vi.mock("@lingui/core/macro", () => ({
  msg: (parts: TemplateStringsArray) => ({ id: parts.join("") }),
}));
vi.mock("@carbon/lib/workflows", () => ({ raiseMoment: vi.fn() }));
vi.mock("@carbon/lib/telemetry", () => ({
  asJobSource: vi.fn(),
  trackWorkEvent: vi.fn(),
}));

import { raiseMoment } from "@carbon/lib/workflows";
import { updateJobStatus } from "../app/modules/production/production.service";

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}

it("reads the guarded Carbon status change through the Rust adapter", async () => {
  const origin = new URL(required("MOM_CARBON_REST_ORIGIN"));
  if (origin.href !== origin.origin + "/" || origin.hostname !== "127.0.0.1") {
    throw new Error("This test requires a dedicated loopback Carbon stack");
  }
  const projectKey = required("MOM_CARBON_PROJECT_KEY");
  const token = required("MOM_SAMPLE_TOKEN");
  const hidden = required("MOM_CARBON_OTHER_JOB_ID");
  const options = { auth: { persistSession: false, autoRefreshToken: false } };
  const admin = createClient(origin.href, required("MOM_CARBON_TEST_SERVICE_KEY"), options);
  const caller = createClient(origin.href, projectKey, {
    ...options,
    global: { headers: { Authorization: `Bearer ${token}` } },
  });

  const actor = await caller.auth.getUser(token);
  expect(actor.error).toBeNull();
  const actorId = actor.data.user!.id;
  const sample = await caller
    .from("jobs")
    .select("itemId,locationId,companyId,unitOfMeasureCode")
    .limit(1)
    .single();
  expect(sample.error).toBeNull();
  const company = sample.data!.companyId;
  expect((await caller.from("jobs").select("id").eq("id", hidden)).data).toEqual([]);

  const fixture = await admin
    .from("job")
    .insert({
      ...sample.data,
      jobId: `adapter-${randomUUID()}`,
      status: "Draft",
      quantity: 1,
      createdBy: actorId,
    })
    .select("id")
    .single();
  expect(fixture.error).toBeNull();
  const id = fixture.data!.id;
  const read = (status: string) => {
    execFileSync(
      required("MOM_CARBON_ADAPTER_SMOKE"),
      ["carbon-rest", origin.origin, id, hidden, status],
      {
        env: {
          MOM_SAMPLE_TOKEN: token,
          MOM_CARBON_COMPANY: company,
          MOM_CARBON_PROJECT_KEY: projectKey,
        },
        stdio: "inherit",
      },
    );
  };

  const update = () =>
    updateJobStatus(caller, {
      id,
      companyId: company,
      status: "Ready",
      updatedBy: actorId,
      fromStatuses: ["Draft", "Planned"],
    });
  try {
    read("Draft");

    const applied = await update();
    expect(applied.error).toBeNull();
    expect(applied.updated).toBe(true);
    read("Ready");

    const repeated = await update();
    expect(repeated.error).toBeNull();
    expect(repeated.updated).toBe(false);
    expect(raiseMoment).toHaveBeenCalledTimes(1);
    read("Ready");
  } finally {
    const removed = await admin.from("job").delete().eq("id", id).eq("companyId", company);
    expect(removed.error).toBeNull();
  }
}, 30_000);
