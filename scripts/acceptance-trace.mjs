const supportedLayers = new Set(["rust-integration", "playwright", "agentnexus", "des-baseline"]);

export function validateTrace({ map, stages, read }) {
  const features = [...read("docs/FEATURES.md").matchAll(/^\| (F\d+) \|/gm)]
    .map((match) => match[1]);
  const expected = new Map();
  for (const phase of stages) {
    const path = `acceptance/${phase}.md`;
    const spec = read(`docs/${path}`);
    const cases = [...spec.matchAll(/^\| (P\d[A-Z]?-\d+) \|/gm)].map((match) => match[1]);
    for (const id of [`${phase}-UI`, ...cases]) {
      if (expected.has(id) || !id.startsWith(`${phase}-`)) {
        throw new Error(`Duplicate or misplaced acceptance case: ${id}`);
      }
      expected.set(id, phase);
    }
  }

  const seen = new Set();
  const covered = new Set();
  for (const { id, phase, spec, features: refs, layers, tests } of map) {
    if (seen.has(id) || !expected.has(id) || expected.get(id) !== phase) {
      throw new Error(`Unknown, duplicate or misplaced mapped case: ${id}`);
    }
    seen.add(id);
    const ui = id.endsWith("-UI");
    const allowedSpec = spec === `acceptance/${phase}.md` || (ui && spec === "TEST_AND_MOCK_PLAN.md");
    if (!allowedSpec || !read(`docs/${spec}`).match(new RegExp(`\\b${id}\\b`))) {
      throw new Error(`Broken acceptance reference: ${id} → ${spec}`);
    }
    if (!Array.isArray(refs) || !refs.length || refs.some((name) => !features.includes(name))) {
      throw new Error(`Broken feature reference: ${id}`);
    }
    refs.forEach((name) => covered.add(name));
    if (!Array.isArray(layers) || !layers.length || new Set(layers).size !== layers.length ||
        (ui && !layers.includes("playwright"))) {
      throw new Error(`Invalid test layers: ${id}`);
    }
    for (const layer of layers) {
      if (!supportedLayers.has(layer) || typeof tests?.[layer] !== "string" || !tests[layer].trim()) {
        throw new Error(`Missing or unsupported test mapping: ${id}/${layer}`);
      }
    }
  }
  for (const id of expected.keys()) {
    if (!seen.has(id)) throw new Error(`Missing acceptance case: ${id}`);
  }
  for (const name of features) {
    if (!covered.has(name)) throw new Error(`Uncovered feature: ${name}`);
  }
}

export function requireBrowserEvidence(report, required) {
  function specs(suite) {
    return [...(suite.specs ?? []), ...(suite.suites ?? []).flatMap(specs)];
  }
  const results = (report.suites ?? []).flatMap(specs);
  for (const [id, title] of required) {
    const matches = results.filter((spec) => spec.title === title);
    const tests = matches[0]?.tests;
    if (matches.length !== 1 || !matches[0].ok || !tests?.length ||
        tests.some((test) => test.status !== "expected" || test.results?.at(-1)?.status !== "passed")) {
      throw new Error(`Missing successful browser evidence for ${id}`);
    }
  }
}
