const TRANSIENT_HTTP_STATUSES = new Set([408, 425, 429]);
const ACCEPTED_HTTP_STATUSES = new Set([401, 403]);

export function redactUrl(value) {
  const url = new URL(value);
  url.hash = "";
  url.search = "";
  return url.toString();
}

export function classifyStatus(status) {
  if ((status >= 200 && status < 400) || ACCEPTED_HTTP_STATUSES.has(status)) {
    return { kind: "success", detail: `HTTP ${status}` };
  }
  if (status >= 500 || TRANSIENT_HTTP_STATUSES.has(status)) {
    return { kind: "transient", detail: `HTTP ${status}` };
  }
  return { kind: "permanent", detail: `HTTP ${status}` };
}

function errorDetail(error) {
  if (!(error instanceof Error)) return "request failed";
  const causeCode = error.cause && typeof error.cause === "object" && "code" in error.cause
    ? String(error.cause.code)
    : null;
  return causeCode ? `${error.name} (${causeCode})` : error.name;
}

export async function checkExternalLink({
  url,
  sources,
  policy,
  request,
  sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
  strictTransient = false,
  critical = false
}) {
  let last = { kind: "transient", detail: "request failed" };
  let attemptsMade = 0;

  for (let attempt = 1; attempt <= policy.attempts; attempt += 1) {
    attemptsMade = attempt;
    try {
      let response = await request(url, "HEAD");
      if (response.status === 405 || response.status === 501) response = await request(url, "GET");
      last = classifyStatus(response.status);
      if (last.kind === "success") return { decision: "pass", kind: last.kind };
      if (last.kind === "permanent") break;
    } catch (error) {
      last = { kind: "transient", detail: errorDetail(error) };
    }

    if (attempt < policy.attempts && policy.backoffMs > 0) {
      await sleep(policy.backoffMs * attempt);
    }
  }

  const decision = last.kind === "permanent" || strictTransient || critical ? "fail" : "warn";
  return {
    decision,
    kind: last.kind,
    message: `${sources.join(", ")}: ${redactUrl(url)} (${last.detail} after ${attemptsMade} ${attemptsMade === 1 ? "attempt" : "attempts"}; ${decision})`
  };
}
