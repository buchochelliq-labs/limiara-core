import { createReadStream, existsSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const contentTypes = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".xml": "application/xml; charset=utf-8"
};

function redirectsFrom(distRoot) {
  return new Map(readFileSync(resolve(distRoot, "_redirects"), "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"))
    .map((line) => {
      const [from, to, status = "302"] = line.split(/\s+/);
      return [from, { to, status: Number(status) }];
    }));
}

function candidateFor(distRoot, pathname) {
  const decoded = decodeURIComponent(pathname);
  const relativePath = decoded === "/" ? "index.html" : decoded.replace(/^\/+/, "");
  const candidates = extname(relativePath)
    ? [relativePath]
    : [relativePath, relativePath + ".html", relativePath + "/index.html"];

  for (const candidate of candidates) {
    const absolute = resolve(distRoot, candidate);
    if (absolute.startsWith(resolve(distRoot) + sep) && existsSync(absolute) && statSync(absolute).isFile()) {
      return absolute;
    }
  }
  return null;
}

export function createStaticServer(distRoot) {
  const redirects = redirectsFrom(distRoot);
  return createServer((request, response) => {
    const pathname = new URL(request.url ?? "/", "http://127.0.0.1").pathname;
    const redirect = redirects.get(pathname);
    if (redirect) {
      response.writeHead(redirect.status, { location: redirect.to });
      response.end();
      return;
    }

    const file = candidateFor(distRoot, pathname);
    const status = file ? 200 : 404;
    const selected = file ?? resolve(distRoot, "404.html");
    response.writeHead(status, { "content-type": contentTypes[extname(selected)] ?? "application/octet-stream" });
    createReadStream(selected).pipe(response);
  });
}

export async function verifyPortableArtifact(distRoot) {
  const server = createStaticServer(distRoot);
  await new Promise((accept) => server.listen(0, "127.0.0.1", accept));
  const address = server.address();
  const origin = "http://127.0.0.1:" + address.port;
  try {
    const checks = [
      ["/", 200, "Limiara"],
      ["/guide/getting-started", 200, "Get started"],
      ["/getting-started", 200, "Get started"],
      ["/not-a-real-document", 404, "404"]
    ];
    for (const [path, expectedStatus, expectedText] of checks) {
      const response = await fetch(origin + path);
      const body = await response.text();
      if (response.status !== expectedStatus || !body.includes(expectedText)) {
        throw new Error(path + " returned " + response.status + " without expected content " + expectedText);
      }
    }
  } finally {
    await new Promise((accept, reject) => server.close((error) => error ? reject(error) : accept()));
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const distRoot = resolve(import.meta.dirname, "../.vitepress/dist");
  if (process.argv.includes("--serve")) {
    const port = Number(process.env.PORT ?? 4175);
    createStaticServer(distRoot).listen(port, "127.0.0.1", () => {
      console.log("Portable documentation artifact available at http://127.0.0.1:" + port);
    });
  } else {
    await verifyPortableArtifact(distRoot);
    console.log("Portable documentation artifact served and verified without Cloudflare.");
  }
}
