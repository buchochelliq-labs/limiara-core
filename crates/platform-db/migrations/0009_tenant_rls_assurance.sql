-- Close the remaining direct-table tenant boundaries. Runtime request paths
-- use platform_app with a transaction-local app.tenant_id; the outbox worker
-- uses only the bounded routines declared below.
ALTER TABLE tenant_plugin_installations ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_plugin_installations FORCE ROW LEVEL SECURITY;
ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;
ALTER TABLE outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox FORCE ROW LEVEL SECURITY;

-- A globally unique tenant-owned identifier leaks a known foreign identifier
-- through unique-constraint errors even when RLS hides the row. Keep identity
-- unique inside each tenant (and inside the NULL/global audit scope) instead.
ALTER TABLE audit_log DROP CONSTRAINT audit_log_pkey;
ALTER TABLE audit_log ALTER COLUMN id SET NOT NULL;
ALTER TABLE audit_log
    ADD CONSTRAINT audit_log_tenant_id_key
    UNIQUE NULLS NOT DISTINCT (tenant_id, id);

ALTER TABLE outbox DROP CONSTRAINT outbox_pkey;
ALTER TABLE outbox ADD PRIMARY KEY (tenant_id, id);

CREATE POLICY tenant_plugin_installations_tenant_isolation
ON tenant_plugin_installations
TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY audit_log_tenant_isolation
ON audit_log
TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY outbox_tenant_isolation
ON outbox
TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

-- SECURITY DEFINER routines execute as the migration owner. FORCE RLS still
-- applies to that owner, so these policies grant only the migration role the
-- cross-tenant access required inside the bounded routines.
CREATE POLICY tenant_plugin_installations_migrator_management
ON tenant_plugin_installations
TO platform_migrator
USING (true)
WITH CHECK (true);

CREATE POLICY audit_log_migrator_management
ON audit_log
TO platform_migrator
USING (true)
WITH CHECK (true);

CREATE POLICY outbox_migrator_management
ON outbox
TO platform_migrator
USING (true)
WITH CHECK (true);

-- Audit and outbox records are append-only to the API role. Outbox state is
-- changed only through claim-owned worker routines.
REVOKE UPDATE, DELETE ON audit_log FROM platform_app;
REVOKE UPDATE, DELETE ON outbox FROM platform_app;

CREATE FUNCTION platform_append_global_plugin_audit(
    p_id uuid,
    p_action text,
    p_target text,
    p_metadata jsonb
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_id IS NULL
       OR p_action IS NULL
       OR p_action NOT IN ('plugin.candidate.accepted', 'plugin.candidate.rejected')
       OR p_target IS NULL
       OR p_target !~ '^plugin:[a-z0-9][a-z0-9.-]{0,99}$'
       OR p_metadata IS NULL
       OR jsonb_typeof(p_metadata) <> 'object' THEN
        RAISE EXCEPTION 'invalid global plugin audit input' USING ERRCODE = '22023';
    END IF;

    INSERT INTO public.audit_log (id, tenant_id, actor_id, action, target, metadata)
    VALUES (p_id, NULL, NULL, p_action, p_target, p_metadata);
END;
$$;

-- Startup recovery is intentionally non-enumerating: callers receive only a
-- count, while the state repair and redacted audit append remain atomic.
CREATE FUNCTION platform_recover_plugin_transitions()
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    recovered_count bigint;
BEGIN
    WITH interrupted AS MATERIALIZED (
        SELECT installation.tenant_id,
               installation.plugin_id,
               installation.state AS interrupted_state,
               installation.transition_id,
               CASE installation.state
                   WHEN 'disabling' THEN 'enabled'
                   ELSE 'disabled'
               END AS recovered_state
        FROM public.tenant_plugin_installations AS installation
        WHERE installation.state IN ('enabling', 'disabling')
        FOR UPDATE
    ), recovered AS (
        UPDATE public.tenant_plugin_installations AS installation
        SET state = interrupted.recovered_state,
            candidate_version = NULL,
            previous_state = NULL,
            transition_id = NULL,
            transition_started_at = NULL,
            last_error_control = 'plugin.transition_interrupted',
            updated_at = now()
        FROM interrupted
        WHERE installation.tenant_id = interrupted.tenant_id
          AND installation.plugin_id = interrupted.plugin_id
        RETURNING installation.tenant_id,
                  installation.plugin_id,
                  interrupted.interrupted_state,
                  interrupted.recovered_state,
                  interrupted.transition_id
    ), appended AS (
        INSERT INTO public.audit_log (
            id, tenant_id, actor_id, action, target, metadata
        )
        SELECT gen_random_uuid(),
               recovered.tenant_id,
               NULL,
               'plugin.transition.recovered',
               'plugin:' || recovered.plugin_id,
               jsonb_build_object(
                   'interrupted_state', recovered.interrupted_state,
                   'recovered_state', recovered.recovered_state,
                   'transition_id', recovered.transition_id
               )
        FROM recovered
        RETURNING 1
    )
    SELECT count(*) INTO recovered_count FROM appended;

    RETURN recovered_count;
END;
$$;

CREATE FUNCTION platform_claim_outbox_batch(
    p_batch_size bigint,
    p_claim_token uuid,
    p_lease_ms bigint
)
RETURNS TABLE (
    id uuid,
    tenant_id uuid,
    event_type text,
    envelope jsonb,
    created_at timestamptz,
    attempt_count integer,
    claim_token uuid,
    claim_expires_at timestamptz
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_batch_size IS NULL
       OR p_batch_size NOT BETWEEN 1 AND 1000
       OR p_claim_token IS NULL
       OR p_lease_ms IS NULL
       OR p_lease_ms NOT BETWEEN 1 AND 3600000 THEN
        RAISE EXCEPTION 'invalid outbox claim input' USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    WITH candidates AS (
        SELECT candidate.tenant_id, candidate.id
        FROM public.outbox AS candidate
        WHERE candidate.published_at IS NULL
          AND candidate.quarantined_at IS NULL
          AND candidate.next_attempt_at <= now()
          AND (
              candidate.claim_expires_at IS NULL
              OR candidate.claim_expires_at <= now()
          )
        ORDER BY candidate.next_attempt_at, candidate.created_at, candidate.id
        FOR UPDATE SKIP LOCKED
        LIMIT p_batch_size
    )
    UPDATE public.outbox AS pending
    SET claim_token = p_claim_token,
        claim_expires_at = now() + (p_lease_ms * INTERVAL '1 millisecond'),
        last_attempt_at = now(),
        attempt_count = pending.attempt_count + 1
    FROM candidates
    WHERE pending.tenant_id = candidates.tenant_id
      AND pending.id = candidates.id
    RETURNING pending.id,
              pending.tenant_id,
              pending.event_type,
              pending.envelope,
              pending.created_at,
              pending.attempt_count,
              pending.claim_token,
              pending.claim_expires_at;
END;
$$;

CREATE FUNCTION platform_mark_outbox_published(
    p_tenant_id uuid,
    p_id uuid,
    p_claim_token uuid
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    affected bigint;
BEGIN
    IF p_tenant_id IS NULL OR p_id IS NULL OR p_claim_token IS NULL THEN
        RAISE EXCEPTION 'invalid outbox settlement input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.outbox
    SET published_at = now(),
        claim_token = NULL,
        claim_expires_at = NULL,
        last_error_code = NULL
    WHERE tenant_id = p_tenant_id
      AND id = p_id
      AND claim_token = p_claim_token
      AND published_at IS NULL
      AND quarantined_at IS NULL;
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

CREATE FUNCTION platform_mark_outbox_failed(
    p_tenant_id uuid,
    p_id uuid,
    p_claim_token uuid,
    p_retry_ms bigint,
    p_error_code text
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    affected bigint;
BEGIN
    IF p_tenant_id IS NULL
       OR p_id IS NULL
       OR p_claim_token IS NULL
       OR p_retry_ms IS NULL
       OR p_retry_ms NOT BETWEEN 1 AND 86400000
       OR p_error_code IS NULL
       OR p_error_code !~ '^[a-z][a-z0-9._-]{0,63}$' THEN
        RAISE EXCEPTION 'invalid outbox failure input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.outbox
    SET failure_count = failure_count + 1,
        last_failure_at = now(),
        last_error_code = p_error_code,
        next_attempt_at = now() + (p_retry_ms * INTERVAL '1 millisecond'),
        claim_token = NULL,
        claim_expires_at = NULL
    WHERE tenant_id = p_tenant_id
      AND id = p_id
      AND claim_token = p_claim_token
      AND published_at IS NULL
      AND quarantined_at IS NULL;
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

CREATE FUNCTION platform_mark_outbox_quarantined(
    p_tenant_id uuid,
    p_id uuid,
    p_claim_token uuid,
    p_error_code text
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    affected bigint;
BEGIN
    IF p_tenant_id IS NULL
       OR p_id IS NULL
       OR p_claim_token IS NULL
       OR p_error_code IS NULL
       OR p_error_code !~ '^[a-z][a-z0-9._-]{0,63}$' THEN
        RAISE EXCEPTION 'invalid outbox quarantine input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.outbox
    SET quarantined_at = now(),
        failure_count = failure_count + 1,
        last_failure_at = now(),
        last_error_code = p_error_code,
        claim_token = NULL,
        claim_expires_at = NULL
    WHERE tenant_id = p_tenant_id
      AND id = p_id
      AND claim_token = p_claim_token
      AND published_at IS NULL
      AND quarantined_at IS NULL;
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

CREATE FUNCTION platform_outbox_backlog()
RETURNS TABLE (
    pending bigint,
    claimable bigint,
    claimed bigint,
    oldest_pending_at timestamptz,
    quarantined bigint
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT count(*),
           count(*) FILTER (
               WHERE queued.next_attempt_at <= now()
                 AND (
                     queued.claim_expires_at IS NULL
                     OR queued.claim_expires_at <= now()
                 )
           ),
           count(*) FILTER (WHERE queued.claim_expires_at > now()),
           min(queued.created_at),
           (SELECT count(*) FROM public.outbox AS quarantined_row
            WHERE quarantined_row.quarantined_at IS NOT NULL)
    FROM public.outbox AS queued
    WHERE queued.published_at IS NULL
      AND queued.quarantined_at IS NULL
$$;

-- Remove legacy blanket function execution from existing installations. Each
-- runtime role receives only the routines needed by its process boundary.
ALTER DEFAULT PRIVILEGES FOR ROLE platform_migrator
    REVOKE EXECUTE ON FUNCTIONS FROM platform_app;
ALTER DEFAULT PRIVILEGES FOR ROLE platform_migrator
    REVOKE EXECUTE ON FUNCTIONS FROM platform_worker;
DO $$
DECLARE
    runtime_schema name;
BEGIN
    FOR runtime_schema IN
        SELECT namespace.nspname
        FROM pg_catalog.pg_namespace AS namespace
        WHERE namespace.nspname NOT LIKE 'pg\_%'
          AND namespace.nspname <> 'information_schema'
    LOOP
        EXECUTE format(
            'REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA %I FROM PUBLIC, platform_app, platform_worker',
            runtime_schema
        );
    END LOOP;
END;
$$;

REVOKE ALL ON FUNCTION platform_append_global_plugin_audit(uuid, text, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_recover_plugin_transitions() FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_claim_outbox_batch(bigint, uuid, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_mark_outbox_published(uuid, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_mark_outbox_failed(uuid, uuid, uuid, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_mark_outbox_quarantined(uuid, uuid, uuid, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_outbox_backlog() FROM PUBLIC;

GRANT EXECUTE ON FUNCTION lookup_ui_external_publication(text) TO platform_app;
GRANT EXECUTE ON FUNCTION platform_append_global_plugin_audit(uuid, text, text, jsonb) TO platform_app;

GRANT SELECT ON plugin_migration_ledger TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_recover_plugin_transitions() TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_claim_outbox_batch(bigint, uuid, bigint) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_mark_outbox_published(uuid, uuid, uuid) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_mark_outbox_failed(uuid, uuid, uuid, bigint, text) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_mark_outbox_quarantined(uuid, uuid, uuid, text) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_outbox_backlog() TO platform_worker;
