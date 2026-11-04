import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { prepareDeployment } from "./prepare-deployment.mjs";

const repositoryRoot = resolve(import.meta.dirname, "../../..");

function assertPublishingWorkflowPolicy(source) {
  const workflow = source.replace(/\r\n?/g, "\n");
  assert.doesNotMatch(workflow, /pull_request_target/);
  assert.match(workflow, /head\.repo\.full_name == github\.repository/);
  assert.match(workflow, /vars\.DOCS_DEPLOY_ENABLED == 'true'/);
  assert.match(workflow, /name: docs-preview/);
  assert.match(workflow, /name: docs-production/);
  assert.match(workflow, /^permissions:\n {2}contents: read$/m);
  assert.match(workflow, /retention-days: 90/);
}

test("preview artifacts are noindex and record disabled data collection", () => {
  const root = mkdtempSync(join(tmpdir(), "limiara-docs-preview-"));
  try {
    prepareDeployment(root, "preview", "abc123");
    assert.match(readFileSync(join(root, "robots.txt"), "utf8"), /Disallow: \//);
    assert.deepEqual(JSON.parse(readFileSync(join(root, "deployment.json"), "utf8")), {
      schemaVersion: 1,
      environment: "preview",
      sourceRevision: "abc123",
      portableStaticSite: true,
      analyticsEnabled: false,
      feedbackEnabled: false
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("production artifacts advertise the canonical sitemap", () => {
  const root = mkdtempSync(join(tmpdir(), "limiara-docs-production-"));
  try {
    prepareDeployment(root, "production", "def456");
    const robots = readFileSync(join(root, "robots.txt"), "utf8");
    assert.match(robots, /Allow: \//);
    assert.match(robots, /Sitemap: https:\/\/docs\.limiara\.org\/sitemap\.xml/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("unknown deployment environments fail closed", () => {
  const root = mkdtempSync(join(tmpdir(), "limiara-docs-invalid-"));
  try {
    assert.throws(() => prepareDeployment(root, "staging"), /Unsupported DOCS_DEPLOYMENT_ENV/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("publishing workflow isolates forks and gates external deployment", () => {
  const workflow = readFileSync(join(repositoryRoot, ".github/workflows/docs-deploy.yml"), "utf8");
  assertPublishingWorkflowPolicy(workflow);
});

test("publishing workflow policy treats LF and CRLF equivalently", () => {
  const workflow = readFileSync(join(repositoryRoot, ".github/workflows/docs-deploy.yml"), "utf8");
  const lf = workflow.replace(/\r\n?/g, "\n");
  const crlf = lf.replaceAll("\n", "\r\n");
  assert.doesNotThrow(() => assertPublishingWorkflowPolicy(lf));
  assert.doesNotThrow(() => assertPublishingWorkflowPolicy(crlf));
});

test("publishing workflow policy rejects weakened security controls", () => {
  const workflow = readFileSync(join(repositoryRoot, ".github/workflows/docs-deploy.yml"), "utf8")
    .replace(/\r\n?/g, "\n");
  const mutations = [
    workflow.replace(/^permissions:\n {2}contents: read\n/m, ""),
    workflow.replaceAll("github.event.pull_request.head.repo.full_name == github.repository", "true"),
    workflow.replaceAll("vars.DOCS_DEPLOY_ENABLED == 'true'", "true")
  ];
  for (const mutation of mutations) {
    assert.throws(() => assertPublishingWorkflowPolicy(mutation), assert.AssertionError);
  }
});

test("static hosting policy defines redirects, caching, and defensive headers", () => {
  const publicRoot = join(repositoryRoot, "docs/public");
  const redirects = readFileSync(join(publicRoot, "_redirects"), "utf8");
  const headers = readFileSync(join(publicRoot, "_headers"), "utf8");
  assert.match(redirects, /^\/getting-started \/guide\/getting-started 301/m);
  assert.match(redirects, /^\/api \/api\/reference 302/m);
  assert.match(headers, /Cache-Control: public, max-age=31536000, immutable/);
  assert.match(headers, /Cache-Control: public, max-age=0, must-revalidate/);
  assert.match(headers, /X-Content-Type-Options: nosniff/);
});
