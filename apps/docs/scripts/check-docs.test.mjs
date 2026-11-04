import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { checkDocumentation } from "./docs-checks.mjs";

function fixture({ markdown = "# Home\n", path = "/health/live", visibility = "public", json } = {}) {
  const root = mkdtempSync(join(tmpdir(), "limiara-docs-"));
  const docsRoot = join(root, "docs");
  const policyRoot = join(root, "policies");
  mkdirSync(join(docsRoot, "api"), { recursive: true });
  mkdirSync(policyRoot, { recursive: true });
  writeFileSync(join(policyRoot, "terminology.json"), JSON.stringify({ rules: [] }));
  writeFileSync(join(docsRoot, "index.md"), markdown);
  const spec = {
    openapi: "3.0.3",
    info: { title: "Test", version: "1" },
    paths: {
      [path]: {
        get: {
          operationId: "getLiveness",
          "x-limiara-visibility": visibility,
          responses: { 200: { description: "ok" } }
        }
      }
    }
  };
  writeFileSync(join(docsRoot, "api/openapi.json"), json ?? JSON.stringify(spec));
  writeFileSync(join(docsRoot, "api/reference.md"), `# API\n\n\`${path}\` \`getLiveness\`\n`);
  writeFileSync(join(docsRoot, "owners.json"), JSON.stringify({
    version: 1,
    owners: { test: { name: "Test", scope: "Fixture pages", requiredReviews: ["test"] } }
  }));
  writeFileSync(join(docsRoot, "inventory.json"), JSON.stringify({
    version: 1,
    entries: ["index.md", "api/reference.md"].map((entryPath) => ({
      path: entryPath,
      title: entryPath,
      audiences: ["contributor"],
      publication: "public",
      productStatus: "experimental",
      owner: "test",
      edition: "not-applicable",
      versioning: "current",
      sensitivity: "public",
      relationship: "canonical"
    }))
  }));
  return { root, docsRoot, policyRoot };
}

function run(options) {
  const value = fixture(options);
  try {
    return checkDocumentation(value);
  } finally {
    rmSync(value.root, { recursive: true, force: true });
  }
}

test("accepts a minimal public documentation set", () => {
  assert.deepEqual(run().failures, []);
});

test("rejects a broken relative link with its source page", () => {
  const result = run({ markdown: "# Home\n\n[Missing](./missing.md)\n" });
  assert.match(result.failures.join("\n"), /index\.md links to missing/);
});

test("rejects malformed OpenAPI JSON", () => {
  const result = run({ json: "{" });
  assert.match(result.failures.join("\n"), /openapi\.json is not valid JSON/);
});

test("rejects internal readiness paths from the public specification", () => {
  const result = run({ path: "/health/ready" });
  assert.match(result.failures.join("\n"), /prohibited internal path \/health\/ready/);
});

test("requires an explicit public visibility declaration", () => {
  const result = run({ visibility: "internal" });
  assert.match(result.failures.join("\n"), /x-limiara-visibility: public/);
});

test("rejects likely credentials without printing the value", () => {
  const result = run({ markdown: "# Home\n\n`AKIAABCDEFGHIJKLMNOP`\n" });
  assert.match(result.failures.join("\n"), /likely AWS access key/);
  assert.doesNotMatch(result.failures.join("\n"), /AKIAABCDEFGHIJKLMNOP/);
});

test("rejects an ungoverned Markdown page", () => {
  const value = fixture();
  try {
    writeFileSync(join(value.docsRoot, "orphan.md"), "# Orphan\n");
    const result = checkDocumentation(value);
    assert.match(result.failures.join("\n"), /orphan\.md has no entry in inventory\.json/);
  } finally {
    rmSync(value.root, { recursive: true, force: true });
  }
});
