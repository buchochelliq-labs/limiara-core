import { resolve } from "node:path";
import { checkDocumentation } from "./docs-checks.mjs";

const docsRoot = resolve(import.meta.dirname, "../../../docs");
const policyRoot = resolve(import.meta.dirname, "../policies");
const result = checkDocumentation({ docsRoot, policyRoot });

if (result.failures.length) {
  console.error(`Documentation checks failed:\n- ${result.failures.join("\n- ")}`);
  process.exit(1);
}

console.log(`Documentation checks passed (${result.markdownCount} Markdown files, ${result.inventoryCount} governed pages, ${result.apiPathCount} public API path).`);
