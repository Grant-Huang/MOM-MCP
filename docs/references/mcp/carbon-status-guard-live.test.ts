// Copy to apps/erp/test in the pinned Carbon checkout. Uses an isolated stack.
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServiceContext } from "@carbon/api/service-fn.server";
import {
	getPostgresClient,
	type KyselyDatabase,
} from "@carbon/database/client";
import { PostgresDriver } from "kysely";
import { Pool } from "pg";
import { createClient } from "@supabase/supabase-js";
import { expect, it, vi } from "vitest";

// Carbon's default Vitest configuration supplies inert service credentials.
// Install the isolated stack bindings before any Carbon module imports them.
vi.hoisted(() => {
	const origin = new URL(process.env.MOM_CARBON_REST_ORIGIN ?? "");
	if (origin.href !== origin.origin + "/" || origin.hostname !== "127.0.0.1") {
		throw new Error("This test requires a dedicated loopback Carbon stack");
	}
	vi.stubEnv("SUPABASE_URL", origin.origin);
	vi.stubEnv("SUPABASE_API_URL", origin.origin);
	vi.stubEnv("SUPABASE_ANON_KEY", process.env.MOM_CARBON_PROJECT_KEY);
	vi.stubEnv(
		"SUPABASE_SERVICE_ROLE_KEY",
		process.env.MOM_CARBON_TEST_SERVICE_KEY,
	);
});

vi.mock("@lingui/core/macro", () => ({
	msg: (parts: TemplateStringsArray) => ({ id: parts.join("") }),
}));
vi.mock("@carbon/lib/workflows", () => ({ raiseMoment: vi.fn() }));
vi.mock("@carbon/lib/telemetry", () => ({
	asJobSource: vi.fn(),
	trackWorkEvent: vi.fn(),
}));

import { raiseMoment } from "@carbon/lib/workflows";
import { releaseJobs } from "../app/modules/production/production.server";
import { updateJobStatus } from "../app/modules/production/production.service";

function required(name: string): string {
	const value = process.env[name];
	if (!value) throw new Error(`Missing ${name}`);
	return value;
}

it.each(["status guard", "release workflow"] as const)(
	"reads Carbon %s through the Rust adapter",
	async (mode) => {
		vi.clearAllMocks();
		const origin = new URL(required("MOM_CARBON_REST_ORIGIN"));
		if (
			origin.href !== origin.origin + "/" ||
			origin.hostname !== "127.0.0.1"
		) {
			throw new Error("This test requires a dedicated loopback Carbon stack");
		}
		const projectKey = required("MOM_CARBON_PROJECT_KEY");
		const token = required("MOM_SAMPLE_TOKEN");
		const hidden = required("MOM_CARBON_OTHER_JOB_ID");
		const options = {
			auth: { persistSession: false, autoRefreshToken: false },
		};
		const admin = createClient(
			origin.href,
			required("MOM_CARBON_TEST_SERVICE_KEY"),
			options,
		);
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
		expect(
			(await caller.from("jobs").select("id").eq("id", hidden)).data,
		).toEqual([]);

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
		let releaseLocationId: string | undefined;
		try {
			read("Draft");

			if (mode === "status guard") {
				const applied = await update();
				expect(applied.error).toBeNull();
				expect(applied.updated).toBe(true);
				read("Ready");

				const repeated = await update();
				expect(repeated.error).toBeNull();
				expect(repeated.updated).toBe(false);
			} else {
				const location = await admin
					.from("location")
					.insert({
						name: `adapter-${randomUUID()}`,
						addressLine1: "Fixture address",
						city: "Fixture city",
						postalCode: "00000",
						timezone: "UTC",
						companyId: company,
						createdBy: actorId,
					})
					.select("id")
					.single();
				expect(location.error).toBeNull();
				releaseLocationId = location.data!.id;
				const assigned = await admin
					.from("job")
					.update({ locationId: releaseLocationId })
					.eq("id", id)
					.eq("companyId", company);
				expect(assigned.error).toBeNull();

				const database = new URL(required("SUPABASE_DB_URL"));
				if (!["localhost", "127.0.0.1"].includes(database.hostname)) {
					throw new Error(
						"This test requires a dedicated loopback PostgreSQL database",
					);
				}
				const pool = new Pool({ connectionString: database.href });
				const pg = getPostgresClient<KyselyDatabase>(pool, PostgresDriver);
				const ctx = createServiceContext({
					actor: {
						type: "user",
						userId: actorId,
						permissions: {
							production: {
								view: [company],
								update: [company],
								create: [],
								delete: [],
							},
						},
					},
					company: { id: company, groupId: company },
					caller,
					system: () => admin,
					pg: () => pg,
					kv: () => ({}) as never,
				});

				const process = await admin
					.from("process")
					.insert({
						name: `adapter-${randomUUID()}`,
						defaultStandardFactor: "Hours/Piece",
						companyId: company,
						createdBy: actorId,
					})
					.select("id")
					.single();
				expect(process.error).toBeNull();
				const processId = process.data!.id;
				const expectBlocked = async (reason: string) => {
					const blocked = await releaseJobs(ctx, { jobIds: [id] });
					expect(blocked.error).toBeNull();
					expect(blocked.data?.released).toEqual([]);
					expect(blocked.data?.failed).toEqual([
						expect.objectContaining({
							jobId: id,
							message: expect.stringContaining(reason),
						}),
					]);
					expect(raiseMoment).not.toHaveBeenCalled();
					const row = await caller
						.from("job")
						.select("releasedDate")
						.eq("id", id)
						.single();
					expect(row.error).toBeNull();
					expect(row.data?.releasedDate).toBeNull();
					read("Draft");
				};
				try {
					await expectBlocked("no operations");
					const method = await admin
						.from("jobMakeMethod")
						.select("id")
						.eq("companyId", company)
						.eq("jobId", id)
						.is("parentMaterialId", null)
						.single();
					expect(method.error).toBeNull();
					const operation = await admin.from("jobOperation").insert({
						jobId: id,
						jobMakeMethodId: method.data!.id,
						processId,
						description: "Adapter release fixture",
						companyId: company,
						createdBy: actorId,
					});
					expect(operation.error).toBeNull();

					const setQuantity = async (quantity: number) => {
						const updated = await admin
							.from("job")
							.update({ quantity })
							.eq("id", id)
							.eq("companyId", company)
							.select("id")
							.single();
						expect(updated.error).toBeNull();
					};
					await setQuantity(0);
					await expectBlocked("nothing to make");
					await setQuantity(1);
					const result = await releaseJobs(ctx, { jobIds: [id] });
					expect(result.error).toBeNull();
					expect(result.data?.failed).toEqual([]);
					expect(result.data?.warnings).toEqual([]);
					expect(result.data?.released.map((job) => job.jobId)).toEqual([id]);
					expect(result.data?.scheduled).toBe(true);
					read("Ready");

					const released = await caller
						.from("job")
						.select("releasedDate")
						.eq("id", id)
						.single();
					expect(released.error).toBeNull();
					expect(released.data?.releasedDate).toBeTruthy();
					const repeated = await releaseJobs(ctx, { jobIds: [id] });
					expect(repeated.error).toBeNull();
					expect(repeated.data?.released).toEqual([]);
					expect(
						repeated.data?.skipped.map((job) => [job.jobId, job.status]),
					).toEqual([[id, "Ready"]]);
				} finally {
					await pg.destroy();
					await admin
						.from("jobOperation")
						.delete()
						.eq("jobId", id)
						.eq("companyId", company);
					expect(
						(
							await admin
								.from("process")
								.delete()
								.eq("id", processId)
								.eq("companyId", company)
						).error,
					).toBeNull();
				}
			}
			expect(raiseMoment).toHaveBeenCalledTimes(1);
			read("Ready");
		} finally {
			const removed = await admin
				.from("job")
				.delete()
				.eq("id", id)
				.eq("companyId", company);
			expect(removed.error).toBeNull();
			if (releaseLocationId) {
				expect(
					(
						await admin
							.from("location")
							.delete()
							.eq("id", releaseLocationId)
							.eq("companyId", company)
					).error,
				).toBeNull();
			}
		}
	},
	60_000,
);
