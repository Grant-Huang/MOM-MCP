// Read-only probe for two authorized Carbon Community test-company sessions.
import { pathToFileURL } from "node:url";

class ProbeError extends Error {}

export async function checkCommunityIsolation({ origin, projectKey, a, b, expiredToken, request = fetch }) {
  if (!URL.canParse(origin)) throw new ProbeError("Invalid Supabase origin");
  const endpoint = new URL(origin);
  const loopback = ["127.0.0.1", "[::1]"].includes(endpoint.hostname);
  if (!(endpoint.protocol === "https:" || endpoint.protocol === "http:" && loopback)
    || endpoint.username || endpoint.password || endpoint.search || endpoint.hash || endpoint.pathname !== "/") {
    throw new ProbeError("Invalid Supabase origin");
  }
  const present = (value) => typeof value === "string" && value.trim().length > 0;
  const id = (value) => typeof value === "string" && /^[A-Za-z0-9_-]+$/.test(value);
  if (!present(projectKey) || !present(a.token) || !present(b.token) || a.token === b.token
    || !id(a.job) || !id(b.job) || a.job === b.job) {
    throw new ProbeError("Provide distinct company sessions and job IDs");
  }

  if (expiredToken !== undefined && (!present(expiredToken) || [a.token, b.token].includes(expiredToken))) {
    throw new ProbeError("Provide a distinct expired test session");
  }

  function readResponse(session, job) {
    const url = new URL("/rest/v1/jobs", endpoint);
    url.search = new URLSearchParams({ select: "id,companyId,jobId,status", id: `eq.${job}`, limit: "2" });
    return request(url, {
      method: "GET",
      redirect: "error",
      headers: { apikey: projectKey, Authorization: `Bearer ${session}`, Accept: "application/json" },
      signal: AbortSignal.timeout(10_000),
    });
  }

  async function read(session, job, label) {
    const response = await readResponse(session, job);
    if (response.status !== 200) throw new ProbeError(`${label} returned HTTP ${response.status}`);
    const rows = await response.json();
    if (!Array.isArray(rows) || rows.length > 1) throw new ProbeError(`${label} returned invalid cardinality`);
    return rows;
  }

  const [ownA] = await read(a.token, a.job, "Company A own-job read");
  const [ownB] = await read(b.token, b.job, "Company B own-job read");
  if (ownA?.id !== a.job || ownB?.id !== b.job || !id(ownA.companyId) || !id(ownB.companyId)
    || ownA.companyId === ownB.companyId
    || ![ownA.jobId, ownA.status, ownB.jobId, ownB.status].every(present)) {
    throw new ProbeError("Own-job reads did not establish two distinct visible jobs and companies");
  }

  const fromA = await read(a.token, b.job, "Company A cross-company read");
  const fromB = await read(b.token, a.job, "Company B cross-company read");
  if (fromA.length || fromB.length) throw new ProbeError("A cross-company job was visible");
  const report = { ownCompany: { a: "record", b: "record" }, otherCompany: { a: "empty", b: "empty" } };
  if (expiredToken !== undefined) {
    const expired = await readResponse(expiredToken, a.job);
    if (expired.status !== 401) {
      throw new ProbeError(`Expired own-job read expected HTTP 401, received ${expired.status}`);
    }
    return { ...report, expiredOwnJob: "unauthorized" };
  }
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const env = process.env;
    console.log(JSON.stringify(await checkCommunityIsolation({
      origin: env.MOM_CARBON_REST_ORIGIN,
      projectKey: env.MOM_CARBON_PROJECT_KEY,
      expiredToken: env.MOM_CARBON_EXPIRED_A_TOKEN,
      a: { token: env.MOM_CARBON_A_TOKEN, job: env.MOM_CARBON_A_JOB_ID },
      b: { token: env.MOM_CARBON_B_TOKEN, job: env.MOM_CARBON_B_JOB_ID },
    })));
  } catch (error) {
    console.error(error instanceof ProbeError ? error.message : "Carbon Community probe failed without safe diagnostics");
    process.exitCode = 1;
  }
}
