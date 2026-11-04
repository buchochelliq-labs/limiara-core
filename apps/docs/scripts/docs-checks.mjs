import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, extname, join, relative, resolve } from "node:path";

const HTTP_METHODS = new Set(["get", "put", "post", "delete", "options", "head", "patch", "trace"]);
const PROHIBITED_PUBLIC_PATHS = [
  /(?:^|\/)ready(?:\/|$)/i,
  /(?:^|\/)(?:internal|admin|debug)(?:\/|$)/i,
  /(?:^|\/)metrics(?:\/|$)/i
];

export const SECRET_PATTERNS = [
  { name: "private key", pattern: /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/ },
  { name: "GitHub token", pattern: /\bgh[pousr]_[A-Za-z0-9_]{30,}\b/ },
  { name: "AWS access key", pattern: /\bAKIA[0-9A-Z]{16}\b/ },
  { name: "OpenAI-style secret", pattern: /\bsk-[A-Za-z0-9_-]{24,}\b/ },
  { name: "bearer token", pattern: /\bBearer\s+[A-Za-z0-9._~-]{24,}\b/i },
  { name: "embedded URL credentials", pattern: /https?:\/\/[^\s/:]+:[^\s/@]+@/i },
  { name: "client secret", pattern: /["']client_secret["']\s*:\s*["'](?!<|\$\{|example|replace|redacted)[^"']{12,}["']/i }
];

export function filesBelow(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? filesBelow(path) : [path];
  });
}

export function sensitiveContentFailures(files, root) {
  const failures = [];
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    for (const rule of SECRET_PATTERNS) {
      if (rule.pattern.test(source)) {
        failures.push(`${relative(root, file)} contains a likely ${rule.name}`);
      }
    }
  }
  return failures;
}

function loadTerminology(policyRoot) {
  const path = join(policyRoot, "terminology.json");
  if (!existsSync(path)) return [];
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  return Array.isArray(parsed.rules) ? parsed.rules : [];
}

function resolveMarkdownTarget(docsRoot, file, target) {
  const decoded = decodeURIComponent(target);
  const candidate = decoded.startsWith("/")
    ? join(docsRoot, decoded.slice(1))
    : resolve(dirname(file), decoded);
  return extname(candidate)
    ? [candidate, join(docsRoot, "public", decoded.replace(/^\//, ""))]
    : [candidate, `${candidate}.md`, join(candidate, "index.md"), join(candidate, "README.md")];
}

function checkMarkdown(docsRoot, markdownFiles, terminology) {
  const failures = [];
  const markdownLink = /(?<!!)\[[^\]]+\]\(([^)]+)\)/g;
  const markdownImage = /!\[([^\]]*)\]\(([^)]+)\)/g;

  for (const file of markdownFiles) {
    const source = readFileSync(file, "utf8");
    const name = relative(docsRoot, file);
    const h1Count = [...source.matchAll(/^#\s+\S/gm)].length;
    const isHome = /^---[\s\S]*?\blayout:\s*home\b[\s\S]*?---/m.test(source);
    if (!isHome && h1Count !== 1) failures.push(`${name} must contain exactly one level-one heading`);
    if (/[^\r\n\t ]+[ \t]+$/m.test(source)) failures.push(`${name} contains trailing whitespace`);
    if (source.includes("{%") || source.includes("<gitbook")) {
      failures.push(`${name} contains vendor-specific GitBook markup`);
    }

    for (const match of source.matchAll(markdownImage)) {
      if (!match[1].trim()) failures.push(`${name} has an image without alternative text`);
    }

    for (const rule of terminology) {
      if (rule.term && source.includes(rule.term)) {
        failures.push(`${name} uses “${rule.term}”; use “${rule.preferred}”`);
      }
    }

    for (const match of source.matchAll(markdownLink)) {
      const target = match[1].trim().split(/[?#]/, 1)[0];
      if (!target || /^(?:https?:|mailto:|tel:)/.test(target)) continue;
      if (!resolveMarkdownTarget(docsRoot, file, target).some(existsSync)) {
        failures.push(`${name} links to missing ${target}`);
      }
    }
  }
  return failures;
}

function checkOpenApi(docsRoot) {
  const failures = [];
  const specPath = join(docsRoot, "api/openapi.json");
  const referencePath = join(docsRoot, "api/reference.md");
  let spec;
  try {
    spec = JSON.parse(readFileSync(specPath, "utf8"));
  } catch {
    return { failures: ["api/openapi.json is not valid JSON"], pathCount: 0 };
  }

  if (!/^3\.(?:0|1)\.\d+$/.test(spec.openapi ?? "")) {
    failures.push("api/openapi.json must declare OpenAPI 3.0.x or 3.1.x");
  }
  if (!spec.info?.title || !spec.info?.version) {
    failures.push("api/openapi.json must declare info.title and info.version");
  }
  if (!spec.paths || typeof spec.paths !== "object" || !Object.keys(spec.paths).length) {
    failures.push("api/openapi.json must contain at least one public path");
  }

  const reference = existsSync(referencePath) ? readFileSync(referencePath, "utf8") : "";
  const operationIds = new Set();
  for (const [path, pathItem] of Object.entries(spec.paths ?? {})) {
    if (PROHIBITED_PUBLIC_PATHS.some((pattern) => pattern.test(path))) {
      failures.push(`api/openapi.json exposes prohibited internal path ${path}`);
    }
    if (!path.startsWith("/")) failures.push(`api/openapi.json path ${path} must start with /`);
    if (!reference.includes(`\`${path}\``)) failures.push(`api/reference.md omits ${path}`);

    for (const [method, operation] of Object.entries(pathItem ?? {})) {
      if (!HTTP_METHODS.has(method.toLowerCase())) continue;
      const label = `${method.toUpperCase()} ${path}`;
      if (!operation || typeof operation !== "object") {
        failures.push(`${label} must be an operation object`);
        continue;
      }
      if (!operation.operationId) failures.push(`${label} must declare operationId`);
      else if (operationIds.has(operation.operationId)) failures.push(`${label} duplicates operationId ${operation.operationId}`);
      else operationIds.add(operation.operationId);
      if (operation["x-limiara-visibility"] !== "public") {
        failures.push(`${label} must declare x-limiara-visibility: public`);
      }
      if (!operation.responses || !Object.keys(operation.responses).some((status) => /^2\d\d$/.test(status))) {
        failures.push(`${label} must declare a successful 2xx response`);
      }
      if (operation.operationId && !reference.includes(`\`${operation.operationId}\``)) {
        failures.push(`api/reference.md omits operationId ${operation.operationId}`);
      }
    }
  }
  return { failures, pathCount: Object.keys(spec.paths ?? {}).length };
}

function readJson(path, label, failures) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    failures.push(`${label} is missing or not valid JSON`);
    return null;
  }
}

function checkInventory(docsRoot, markdownFiles) {
  const failures = [];
  const inventory = readJson(join(docsRoot, "inventory.json"), "inventory.json", failures);
  const ownership = readJson(join(docsRoot, "owners.json"), "owners.json", failures);
  if (!inventory || !ownership) return { failures, entryCount: 0 };

  const allowed = {
    audience: new Set(["user", "administrator", "operator", "contributor", "plugin-developer", "architect", "security-reviewer"]),
    publication: new Set(["public", "repository-only", "excluded"]),
    productStatus: new Set(["shipped", "experimental", "planned", "mixed", "historical", "not-applicable"]),
    edition: new Set(["community", "commercial", "both", "not-applicable"]),
    versioning: new Set(["current", "version-agnostic", "historical"]),
    sensitivity: new Set(["public", "security-design", "internal"]),
    relationship: new Set(["canonical", "supporting", "historical", "duplicate", "obsolete"])
  };
  const sourcePaths = new Set(markdownFiles.map((file) => relative(docsRoot, file).replaceAll("\\", "/")));
  const entries = Array.isArray(inventory.entries) ? inventory.entries : [];
  const entriesByPath = new Map();
  const normalizedTitles = new Map();

  for (const entry of entries) {
    const label = entry.path || "<missing path>";
    if (!entry.path || entriesByPath.has(entry.path)) failures.push(`inventory.json contains duplicate or missing path ${label}`);
    else entriesByPath.set(entry.path, entry);
    if (!sourcePaths.has(entry.path)) failures.push(`inventory.json references missing Markdown page ${label}`);
    if (!entry.title?.trim()) failures.push(`${label} inventory entry requires a title`);
    if (!Array.isArray(entry.audiences) || !entry.audiences.length || entry.audiences.some((value) => !allowed.audience.has(value))) {
      failures.push(`${label} inventory entry has invalid audiences`);
    }
    for (const field of ["publication", "productStatus", "edition", "versioning", "sensitivity", "relationship"]) {
      if (!allowed[field].has(entry[field])) failures.push(`${label} inventory entry has invalid ${field}`);
    }
    if (!ownership.owners?.[entry.owner]) failures.push(`${label} inventory entry has unknown owner ${entry.owner ?? "<missing>"}`);
    if (entry.publication === "public" && entry.sensitivity === "internal") {
      failures.push(`${label} cannot publish material classified as internal`);
    }
    if (entry.publication === "public" && entry.sensitivity === "security-design" && (entry.publicationRationale?.length ?? 0) < 40) {
      failures.push(`${label} needs a security publication rationale of at least 40 characters`);
    }
    if (["duplicate", "obsolete"].includes(entry.relationship) && !entry.replacedBy) {
      failures.push(`${label} must name replacedBy when relationship is ${entry.relationship}`);
    }
    if (entry.productStatus === "historical" && entry.relationship !== "historical") {
      failures.push(`${label} with historical product status must have a historical relationship`);
    }
    const normalizedTitle = entry.title?.trim().toLocaleLowerCase("en-GB");
    if (normalizedTitle) {
      const existing = normalizedTitles.get(normalizedTitle);
      if (existing && !["duplicate", "obsolete"].includes(entry.relationship)) {
        failures.push(`${label} duplicates title “${entry.title}” from ${existing} without a duplicate decision`);
      } else normalizedTitles.set(normalizedTitle, label);
    }
  }

  for (const path of sourcePaths) {
    if (!entriesByPath.has(path)) failures.push(`${path} has no entry in inventory.json`);
  }
  for (const [owner, definition] of Object.entries(ownership.owners ?? {})) {
    if (!definition.name || !definition.scope || !Array.isArray(definition.requiredReviews) || !definition.requiredReviews.length) {
      failures.push(`owners.json owner ${owner} is missing name, scope, or requiredReviews`);
    }
  }

  return { failures, entryCount: entries.length };
}

export function checkDocumentation({ docsRoot, policyRoot }) {
  const markdownFiles = filesBelow(docsRoot).filter((path) => extname(path) === ".md");
  const scannableFiles = filesBelow(docsRoot).filter((path) => [".md", ".json", ".yaml", ".yml", ".txt", ".svg"].includes(extname(path)));
  const failures = [
    ...checkMarkdown(docsRoot, markdownFiles, loadTerminology(policyRoot)),
    ...sensitiveContentFailures(scannableFiles, docsRoot)
  ];
  const api = checkOpenApi(docsRoot);
  failures.push(...api.failures);
  const inventory = checkInventory(docsRoot, markdownFiles);
  failures.push(...inventory.failures);
  return { failures, markdownCount: markdownFiles.length, apiPathCount: api.pathCount, inventoryCount: inventory.entryCount };
}
