import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function prepareDeployment(distRoot, environment = "local", sourceRevision = "local") {
  if (!new Set(["local", "preview", "production"]).has(environment)) {
    throw new Error(`Unsupported DOCS_DEPLOYMENT_ENV: ${environment}`);
  }

  const preview = environment === "preview";
  const robots = preview
    ? "User-agent: *\nDisallow: /\n"
    : "User-agent: *\nAllow: /\n\nSitemap: https://docs.limiara.org/sitemap.xml\n";

  writeFileSync(resolve(distRoot, "robots.txt"), robots);
  writeFileSync(resolve(distRoot, "deployment.json"), `${JSON.stringify({
    schemaVersion: 1,
    environment,
    sourceRevision,
    portableStaticSite: true,
    analyticsEnabled: false,
    feedbackEnabled: false
  }, null, 2)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  prepareDeployment(
    resolve(import.meta.dirname, "../.vitepress/dist"),
    process.env.DOCS_DEPLOYMENT_ENV ?? "local",
    process.env.DOCS_SOURCE_REVISION ?? "local"
  );
}
