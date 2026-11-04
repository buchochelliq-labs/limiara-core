# 0023: Integration APIs

## Status

Accepted (2026-10-12). Builds on ADR 0015 (object engine), ADR 0018
(change data capture) and ADR 0022 (automation). Part of epic #292 (task:
integration APIs). Composite requests are implemented in
`platform_objects::composite` and `POST /api/v1/workspaces/{id}/composite`.
Bulk jobs (§2) are implemented in `platform_objects::bulk`, PostgreSQL
migration 0025, SQLite tenant step 0011 and the worker. API clients (§3)
are implemented in the API's `api_clients` module and PostgreSQL migration
0026. §3 changes the authentication boundary: it adds a second way to
authenticate workspace requests, for machines only.

## Context

Integrations talk to Salesforce through a few APIs beyond single-record
REST:
- composite requests make several related changes in one round trip,
  optionally all or none;
- bulk jobs load or change thousands of records in the background;
- connected apps let machines authenticate without a person;
- change data capture keeps external systems in sync.

Limiara has change data capture and `touched_at` sync (ADR 0018). The rest
must keep the engine's rules: every change has the caller's permissions and
sharing, and gets the same validation, automations, evidence and history
as a change made one at a time.

## Decision

### 1. Composite requests

`POST /composite` takes up to 25 record operations: `create`, `update`
(with `expected_version`), `delete`, and `undelete` and `change_owner`
(2026-10-20, so a synced device can push a restore or a transfer, ADR 0024
§3). Each has a `reference` that is
unique in the request.

- **References.** A later operation can use `"@{reference.id}"`, as a whole
  string value or as `id`, for the public ID of the record an earlier
  operation created or changed. Using an operation that failed, or that
  comes later, is `composite.reference_unknown` on that operation. Only
  whole values are replaced; text that merely contains the pattern is left
  as written.
- **Independent by default.** Each operation runs in its own transaction,
  exactly as the single-record route would. A failure does not stop later
  operations.
- **All or none** (`"all_or_none": true`). Every operation runs in one
  transaction. The first failure rolls everything back:
  - earlier operations report `composite.rolled_back`;
  - later ones report `composite.not_run`;
  - automation runs that failed are still recorded (ADR 0022 §5).
- **Response.** One result per operation, in order. Each has the status and
  body the single-record route would have returned: the record with
  formula values, or the problem with its stable code.
- **Same rules as single changes.** Each operation has the caller's object
  permissions and sharing. Validation, automations, evidence, change data
  capture and history apply as if the changes were made one at a time.
  Internally the record operations are now steps that run inside a
  caller's transaction, and the single-record routes use the same steps.

### 2. Bulk jobs (amendment)

Added 2026-10-12.

`POST /bulk` queues up to 5,000 rows for one object and one operation:

- `insert` rows are the values of a new record;
- `update` rows are `{"id", "values", "expected_version"?}`;
- `delete` rows are `{"id", "expected_version"?}`.

- **Checked when submitted.** The submitter needs the access the operation
  needs on the object: create, edit or delete. Malformed rows reject the
  whole job, with `bulk.row_invalid` against `rows[i]`; an empty or
  oversized job is `bulk.rows_invalid`. The job and its rows are stored
  with `tenant_id` and forced row-level security, and the submission is
  audited and published as `platform.objects.bulk-job-submitted.v1`.
- **Run in the background, one change per row.** The worker finds jobs
  with work across workspaces only through the bounded
  `platform_due_bulk_jobs` routine, as for automation jobs (ADR 0022 §6).
  It claims a job in its workspace with a 10-minute lock and applies up to
  200 pending rows, then hands the job back so other jobs get a turn.
  - Each row runs in a transaction of its own, through the same steps as
    the single-record routes, so object permissions, sharing, validation,
    automations, evidence, change data capture and history all apply. Its
    events carry the job's id as their cause.
  - The row's result is written in the same transaction as its change, so
    a worker that stops mid-batch never applies a row twice.
  - A row that fails is rolled back and its problem written on its own,
    with the code the single-record route would return. Other rows carry
    on.
  - Without `expected_version`, a row uses the record's current version,
    as Salesforce bulk updates do; with it, a stale row fails with
    `records.version_conflict`.
- **The submitter's current access.** Each batch runs as the member who
  submitted the job, with the permissions, roles and reporting line the
  kernel resolves for them at that moment, never a copy taken at
  submission. A submitter who is no longer a member aborts the job at its
  next batch and nothing more is written.
- **Progress and results.** The submitter, and administrators with
  `objects.manage`, read the job's status and counts at `GET /bulk/{id}`
  and its rows, in order and optionally by status, at
  `GET /bulk/{id}/results`. Others get `404`.
- **Abort.** `POST /bulk/{id}/abort` stops a queued or running job. Rows
  already applied keep their changes and the rest stay pending; anything
  else is `409 bulk.job_state`. Completion and aborts are audited and
  published as `platform.objects.bulk-job-completed.v1` and
  `-aborted.v1`. Job events carry counts, never row values.

### 3. API clients: OAuth 2.0 client credentials (amendment)

Added 2026-10-12.

Browser sign-in stays OIDC authorization code with PKCE and
backend-managed sessions. Integrations get **API clients**, like
Salesforce connected apps using the client-credentials flow. People still
have no passwords.

- **An API client acts as one workspace member.** An administrator with
  the new `api_clients.manage` permission creates a client with a name. It
  acts as the administrator, or as another member if the administrator
  also holds `members.manage`. A request made with the client's token has
  exactly that member's current permissions, roles and sharing, and
  evidence names that member. There is no separate machine identity that
  could hold access no member holds.
- **Secrets are shown once and stored as hashes.** Creating a client
  returns a random public `client_id` (16 bytes) and a random
  `client_secret` (32 bytes). Only the secret's SHA-256 hash is stored,
  which is sufficient for random values of that size. A workspace has at
  most 20 active clients.
- **Token endpoint.** `POST /oauth/token` takes
  `grant_type=client_credentials`, with the client's id and secret in HTTP
  Basic or in the form (RFC 6749 §4.4). It returns an opaque bearer token
  that lasts an hour, stored only as a hash. Errors use the OAuth codes
  `invalid_client`, `invalid_request` and `unsupported_grant_type`, with
  `Cache-Control: no-store`.
  - The endpoint runs before any workspace is known, so the client is
    found only through the bounded SECURITY DEFINER routine
    `platform_issue_api_token`.
  - That routine answers only when the secret matches, the client is not
    revoked and its member still belongs to the workspace.
  - It audits each issued token in the client's workspace.
- **Bearer tokens on object engine routes only.**
  - A request with `Authorization: Bearer` is authenticated by the token
    alone. Any session cookie is ignored, so no request is authenticated
    by both.
  - The token is resolved in its own workspace through
    `platform_api_token_access`, the counterpart of the session routine.
  - Tokens reach only the object engine's routes: objects, records,
    metadata, `query`, `search`, `composite`, `bulk` and device `sync`
    (ADR 0024). Anything else,
    including managing API clients, members, plugins and the UI, is
    `403 auth.client_route_denied`.
- **Revocation.** Revoking a client deletes its tokens at once and stops
  new ones. A client whose member leaves the workspace stops working the
  same way. Creating and revoking clients are audited.
- Both tables, `api_clients` and `api_client_tokens`, carry `tenant_id`
  with forced row-level security.

## Alternatives considered

- **Arbitrary sub-requests to any route** (Salesforce's composite accepts
  most REST calls). This needs an internal router and per-route semantics
  for references and rollback. Record operations cover the integration
  cases and keep all-or-none meaningful.
- **Bulk rows in one transaction per batch.** Faster, but one bad row
  would roll back its neighbours, and a stopped worker would leave a batch
  half known. Salesforce also reports each row on its own.
- **Run bulk jobs with the access captured at submission.** That would let
  a removed member's job keep writing. Resolving access per batch costs
  two queries per batch.
- **API keys that never expire.** Simpler for integrators, but a leaked
  key works until someone notices. Short-lived tokens behind a secret
  follow OAuth practice, and every token is audited.
- **Machine users with their own roles.** This would need a second kind of
  identity throughout membership, sharing and evidence. Acting as a member
  reuses all of it, and the run-as member is chosen deliberately.
- **JWT access tokens.** They avoid a lookup per request but cannot be
  revoked before they expire. Opaque tokens make revocation immediate.
- **Templated references inside text.** Replacing only whole values keeps
  references unambiguous and stops record data from being reinterpreted.

## Consequences

- Integrations can create a parent and its children in one call, either
  atomically or independently.
- An all-or-none request holds one transaction for up to 25 changes and
  their automations, which bounds its cost the same way the automation
  limits do.
- Large loads go through the same validation and automations as members'
  changes. They are slower than raw inserts, but nothing loaded can break
  the engine's rules.
- Bulk jobs need the worker's `platform.worker.tenant_database_url`, as
  automation jobs do. Without it, jobs stay queued.
- Integrations authenticate without a person and without passwords. A
  client can do exactly what its member can, through the engine's routes
  only.
- Left open, in the roadmap:
  - scopes narrower than the member's access;
  - rate limits per client;
  - an administrator page for clients;
  - naming the client, as well as its member, in record evidence.
