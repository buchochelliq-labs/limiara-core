import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";
import { filesBelow, sensitiveContentFailures } from "./docs-checks.mjs";

const distRoot = resolve(import.meta.dirname, "../.vitepress/dist");
const failures = [];
const required = ["index.html", "404.html", "sitemap.xml", "robots.txt", "_redirects", "_headers", "deployment.json"];

for (const name of required) {
  if (!existsSync(join(distRoot, name))) failures.push(`built output is missing ${name}`);
}

const files = filesBelow(distRoot);
const totalBytes = files.reduce((total, file) => total + statSync(file).size, 0);
const sizes = files.map((file) => ({ file, bytes: statSync(file).size }));
const largest = sizes.slice().sort((a, b) => b.bytes - a.bytes)[0];
// The local search index grows with every page and is fetched only when a
// reader opens search, so it has its own budget. Every other asset keeps the
// stricter per-asset budget that protects first-page load.
const ASSET_BUDGET = 300_000;
const SEARCH_INDEX_BUDGET = 600_000;
const isSearchIndex = (file) => /(^|[\\/])@localSearchIndex[^\\/]*\.js$/.test(file);
// The whole site grows with every page (about 30-60 KB per ADR including its
// search entries); the per-asset budgets below keep first-page load in check,
// so the total only guards against accidental bulk such as stray media.
// Raised from 6 MB at 84 pages (ADR 0032), when page growth alone reached it.
const TOTAL_BUDGET = 7_000_000;
if (totalBytes > TOTAL_BUDGET) failures.push(`built output is ${totalBytes} bytes; budget is ${TOTAL_BUDGET}`);
for (const { file, bytes } of sizes) {
  const budget = isSearchIndex(file) ? SEARCH_INDEX_BUDGET : ASSET_BUDGET;
  const kind = isSearchIndex(file) ? "search-index" : "per-asset";
  if (bytes > budget) failures.push(`${relative(distRoot, file)} is ${bytes} bytes; ${kind} budget is ${budget}`);
}

const scannable = files.filter((file) => [".html", ".js", ".json", ".xml", ".txt"].includes(extname(file)) || file.endsWith("_redirects"));
failures.push(...sensitiveContentFailures(scannable, distRoot));

const deployment = JSON.parse(readFileSync(join(distRoot, "deployment.json"), "utf8"));
const index = readFileSync(join(distRoot, "index.html"), "utf8");
const robots = readFileSync(join(distRoot, "robots.txt"), "utf8");
const redirects = readFileSync(join(distRoot, "_redirects"), "utf8");
const headers = readFileSync(join(distRoot, "_headers"), "utf8");
const sitemap = readFileSync(join(distRoot, "sitemap.xml"), "utf8");

for (const marker of ['property="og:title"', 'property="og:description"', 'property="og:url"', 'name="twitter:card"']) {
  if (!index.includes(marker)) failures.push(`home page is missing social metadata ${marker}`);
}
if (!redirects.includes("/getting-started /guide/getting-started 301")) failures.push("redirect policy is missing the permanent getting-started redirect");
if (!headers.includes("Cache-Control: public, max-age=31536000, immutable")) failures.push("cache policy is missing immutable asset caching");
if (!headers.includes("X-Content-Type-Options: nosniff")) failures.push("security headers are missing nosniff");
if (!sitemap.includes("https://docs.limiara.org/")) failures.push("sitemap does not use the approved production origin");

if (deployment.environment === "preview") {
  if (!robots.includes("Disallow: /")) failures.push("preview robots policy must disallow crawling");
  if (!index.includes('name="robots" content="noindex, nofollow"')) failures.push("preview pages must include a noindex meta directive");
} else if (!robots.includes("Allow: /")) {
  failures.push(`${deployment.environment} robots policy must allow crawling`);
}

for (const prohibited of ["static.cloudflareinsights.com", "plausible.io/js", "googletagmanager.com/gtag", "docs-feedback-widget"]) {
  if (index.includes(prohibited)) failures.push(`analytics or feedback integration is not approved: ${prohibited}`);
}

if (failures.length) {
  console.error(`Built documentation checks failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}

console.log(`Built documentation checks passed (${files.length} files, ${totalBytes} bytes; largest asset ${largest?.bytes ?? 0} bytes).`);
