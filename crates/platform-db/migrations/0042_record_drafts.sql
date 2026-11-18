-- Record drafts (ADR 0036, slice 1). Drafts themselves never reach the
-- server: a browser keeps them in IndexedDB, encrypted. The server holds
-- two small things only:
--
-- 1. The workspace's draft expiry, as object engine metadata: one row per
--    workspace, versioned, audited and published like other engine
--    settings. No row means the default of 7 days.
-- 2. One random key per user and browser that the browser derives its
--    per-workspace draft keys from, so drafts left in a browser profile are
--    unreadable without a live session of the same user, and unreadable
--    for good once sign-out deletes the key.

CREATE TABLE record_draft_settings (
    tenant_id uuid NOT NULL PRIMARY KEY REFERENCES tenants(id),
    expiry_days integer NOT NULL CHECK (expiry_days BETWEEN 1 AND 90),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid
);

ALTER TABLE record_draft_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_draft_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY record_draft_settings_tenant_isolation ON record_draft_settings TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_draft_settings_migrator_management ON record_draft_settings TO platform_migrator
    USING (true) WITH CHECK (true);

-- User-scoped, like sessions: not tenant data, and holding no record data.
-- One key serves every workspace in that browser; the browser derives a
-- separate AES-256-GCM key per workspace from it. Keys are never logged
-- and are returned only to a browser session of their own user.
CREATE TABLE draft_keys (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    browser_id uuid NOT NULL,
    key bytea NOT NULL CHECK (octet_length(key) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, browser_id)
);

-- The least recently used keys of a user, for the per-user cap, and every
-- key not used for a while, for the retention job.
CREATE INDEX draft_keys_user_last_used_idx ON draft_keys (user_id, last_used_at);
CREATE INDEX draft_keys_last_used_idx ON draft_keys (last_used_at);

-- Deletes keys unused for longer than p_unused_days (the longest allowed
-- draft expiry, 90 days), in bounded batches. The worker role has no table
-- privileges of its own; this routine is its only access to the keys and
-- it returns no key material.
CREATE FUNCTION platform_purge_draft_keys(p_unused_days integer, p_limit integer)
RETURNS bigint
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    WITH due AS (
        SELECT user_id, browser_id
        FROM public.draft_keys
        WHERE last_used_at < now() - make_interval(days => greatest(p_unused_days, 1))
        ORDER BY last_used_at
        LIMIT least(greatest(p_limit, 1), 5000)
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM public.draft_keys AS k
        USING due
        WHERE k.user_id = due.user_id AND k.browser_id = due.browser_id
        RETURNING 1
    )
    SELECT count(*) FROM deleted
$$;

REVOKE ALL ON FUNCTION platform_purge_draft_keys(integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_purge_draft_keys(integer, integer) TO platform_worker;
