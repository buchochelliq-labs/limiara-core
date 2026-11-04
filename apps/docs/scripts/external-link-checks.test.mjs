import assert from "node:assert/strict";
import test from "node:test";
import { checkExternalLink, classifyStatus, redactUrl } from "./external-link-checks.mjs";

const policy = { attempts: 2, backoffMs: 1 };
const base = {
  url: "https://example.test/guide?token=secret#section",
  sources: ["docs/guide/example.md"],
  policy,
  sleep: async () => {}
};

test("classifies success, permanent failures, and transient responses", () => {
  assert.equal(classifyStatus(301).kind, "success");
  assert.equal(classifyStatus(404).kind, "permanent");
  assert.equal(classifyStatus(429).kind, "transient");
  assert.equal(classifyStatus(503).kind, "transient");
});

test("accepts a successful response", async () => {
  const result = await checkExternalLink({ ...base, request: async () => ({ status: 204 }) });
  assert.equal(result.decision, "pass");
});

test("follows the fetch redirect result and accepts it", async () => {
  const result = await checkExternalLink({ ...base, request: async () => ({ status: 302 }) });
  assert.equal(result.decision, "pass");
});

test("falls back to GET when HEAD is unsupported", async () => {
  const methods = [];
  const result = await checkExternalLink({
    ...base,
    request: async (_url, method) => {
      methods.push(method);
      return { status: method === "HEAD" ? 405 : 200 };
    }
  });
  assert.equal(result.decision, "pass");
  assert.deepEqual(methods, ["HEAD", "GET"]);
});

test("fails a stable broken link with a source-aware redacted diagnostic", async () => {
  const result = await checkExternalLink({ ...base, request: async () => ({ status: 404 }) });
  assert.equal(result.decision, "fail");
  assert.match(result.message, /docs\/guide\/example\.md/);
  assert.match(result.message, /HTTP 404 after 1 attempt/);
  assert.doesNotMatch(result.message, /secret/);
});

for (const [name, request] of [
  ["timeout", async () => { throw new DOMException("timed out", "TimeoutError"); }],
  ["DNS or socket error", async () => { throw new TypeError("fetch failed", { cause: { code: "ENOTFOUND" } }); }],
  ["server error", async () => ({ status: 503 })]
]) {
  test(`warns after retry exhaustion for a transient ${name}`, async () => {
    let attempts = 0;
    const result = await checkExternalLink({
      ...base,
      request: async (...args) => { attempts += 1; return request(...args); }
    });
    assert.equal(result.decision, "warn");
    assert.equal(attempts, 2);
  });
}

test("strict live audits fail after transient retry exhaustion", async () => {
  const result = await checkExternalLink({
    ...base,
    strictTransient: true,
    request: async () => { throw new TypeError("fetch failed"); }
  });
  assert.equal(result.decision, "fail");
});

test("release-critical links fail closed on transient errors", async () => {
  const result = await checkExternalLink({
    ...base,
    critical: true,
    request: async () => ({ status: 503 })
  });
  assert.equal(result.decision, "fail");
});

test("redacts query values and fragments", () => {
  assert.equal(redactUrl(base.url), "https://example.test/guide");
});
