import { readFileSync } from "node:fs";
import { extname, relative, resolve } from "node:path";
import { filesBelow } from "./docs-checks.mjs";
import { checkExternalLink } from "./external-link-checks.mjs";

const docsRoot = resolve(import.meta.dirname, "../../../docs");
const policy = JSON.parse(readFileSync(resolve(import.meta.dirname, "../policies/external-links.json"), "utf8"));
const ignored = policy.ignored.map((entry) => ({ ...entry, expression: new RegExp(entry.pattern) }));
const rawLinks = new Map();

for (const file of filesBelow(docsRoot).filter((path) => extname(path) === ".md")) {
  const source = readFileSync(file, "utf8");
  for (const match of source.matchAll(/https?:\/\/[^\s<>"')\]]+/g)) {
    try {
      const url = new URL(match[0].replace(/[.,;:]$/, ""));
      url.hash = "";
      const value = url.toString();
      const sources = rawLinks.get(value) ?? new Set();
      sources.add(relative(resolve(docsRoot, ".."), file).replaceAll("\\", "/"));
      rawLinks.set(value, sources);
    } catch {
      // Malformed links are handled by the Markdown source checker where applicable.
    }
  }
}

const links = [...rawLinks.entries()]
  .filter(([url]) => !ignored.some((entry) => entry.expression.test(url)))
  .sort(([left], [right]) => left.localeCompare(right));
const strictTransient = process.argv.includes("--strict-transient");
const critical = (policy.critical ?? []).map((pattern) => new RegExp(pattern));

async function request(url, method) {
  return fetch(url, {
    method,
    redirect: "follow",
    signal: AbortSignal.timeout(policy.timeoutMs),
    headers: {
      "User-Agent": "Limiara-docs-link-check/1.0",
      ...(method === "GET" ? { Range: "bytes=0-0" } : {})
    }
  });
}

let cursor = 0;
const failures = [];
const warnings = [];
async function worker() {
  while (cursor < links.length) {
    const index = cursor;
    cursor += 1;
    const [url, sources] = links[index];
    const result = await checkExternalLink({
      url,
      sources: [...sources].sort(),
      policy,
      request,
      strictTransient,
      critical: critical.some((expression) => expression.test(url))
    });
    if (result.decision === "fail") failures.push(result.message);
    if (result.decision === "warn") warnings.push(result.message);
  }
}

await Promise.all(Array.from({ length: Math.min(policy.concurrency, links.length) }, () => worker()));

if (warnings.length) {
  console.warn(`External documentation link warnings:\n- ${warnings.sort().join("\n- ")}`);
}

if (failures.length) {
  console.error(`External documentation link checks failed:\n- ${failures.sort().join("\n- ")}`);
  process.exit(1);
}

console.log(`External documentation link checks passed (${links.length} checked, ${rawLinks.size - links.length} allowlisted, ${warnings.length} transient warnings).`);
