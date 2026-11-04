-- Tenant-scoped schedules for plugin-contributed recurring jobs.
--
-- A schedule row is the single effective registration for one
-- tenant/plugin/job. Disabling a plugin never deletes it: the checkpoint,
-- counters, and next run survive so re-enable resumes from committed state.
-- Claims are leases; completion is committed in the same tenant transaction as
-- the job's side effects and only while the lease is still held.
CREATE TABLE plugin_job_schedules (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    plugin_id text NOT NULL REFERENCES plugin_catalogue(plugin_id),
    job_id text NOT NULL,
    interval_seconds integer NOT NULL CHECK (interval_seconds BETWEEN 60 AND 604800),
    next_run_at timestamptz NOT NULL DEFAULT now(),
    claim_token uuid,
    claim_expires_at timestamptz,
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    run_count bigint NOT NULL DEFAULT 0 CHECK (run_count >= 0),
    checkpoint jsonb NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(checkpoint) = 'object' AND octet_length(checkpoint::text) <= 16384),
    last_started_at timestamptz,
    last_succeeded_at timestamptz,
    last_failed_at timestamptz,
    last_error_code text CHECK (last_error_code IS NULL OR last_error_code ~ '^[a-z][a-z0-9._-]{0,63}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, plugin_id, job_id),
    CHECK (octet_length(job_id) BETWEEN 1 AND 100),
    CHECK (job_id ~ '^[a-z0-9][a-z0-9.-]*[a-z0-9]$'),
    CHECK (left(job_id, octet_length(plugin_id) + 1) = plugin_id || '.'),
    CHECK ((claim_token IS NULL) = (claim_expires_at IS NULL))
);

CREATE INDEX plugin_job_schedules_due_idx
    ON plugin_job_schedules (plugin_id, job_id, next_run_at)
    WHERE claim_token IS NULL;

ALTER TABLE plugin_job_schedules ENABLE ROW LEVEL SECURITY;
ALTER TABLE plugin_job_schedules FORCE ROW LEVEL SECURITY;

CREATE POLICY plugin_job_schedules_tenant_isolation
ON plugin_job_schedules
TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE POLICY plugin_job_schedules_migrator_management
ON plugin_job_schedules
TO platform_migrator
USING (true)
WITH CHECK (true);

-- Tenant request code may observe its schedules but never their claim tokens;
-- every state change goes through the lease-checked routines below.
REVOKE ALL ON plugin_job_schedules FROM platform_app;
GRANT SELECT (
    tenant_id, plugin_id, job_id, interval_seconds, next_run_at, claim_expires_at,
    attempt_count, run_count, checkpoint, last_started_at, last_succeeded_at,
    last_failed_at, last_error_code, created_at, updated_at
) ON plugin_job_schedules TO platform_app;

-- The bounded routines below read installations across tenants. They run as
-- the migration owner through the platform_migrator management policies from
-- 0009; the worker itself still has no direct tenant-table access.

-- Create one schedule per enabled, health-checked tenant installation. Rows
-- are never deleted here, so a disabled installation keeps its schedule and
-- checkpoint. Returns the number of newly registered schedules.
CREATE FUNCTION platform_ensure_plugin_job_schedules(
    p_plugin_id text,
    p_job_id text,
    p_interval_seconds integer
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    registered bigint;
BEGIN
    IF p_plugin_id IS NULL
       OR p_job_id IS NULL
       OR left(p_job_id, octet_length(p_plugin_id) + 1) <> p_plugin_id || '.'
       OR p_interval_seconds IS NULL
       OR p_interval_seconds NOT BETWEEN 60 AND 604800 THEN
        RAISE EXCEPTION 'invalid plugin job registration input' USING ERRCODE = '22023';
    END IF;

    -- Replicas reconcile concurrently; serialise per job so the cross-tenant
    -- interval update cannot deadlock.
    PERFORM pg_advisory_xact_lock(hashtextextended('plugin-job:' || p_plugin_id || ':' || p_job_id, 0));

    UPDATE public.plugin_job_schedules AS schedule
    SET interval_seconds = p_interval_seconds,
        updated_at = now()
    WHERE schedule.plugin_id = p_plugin_id
      AND schedule.job_id = p_job_id
      AND schedule.interval_seconds <> p_interval_seconds;

    INSERT INTO public.plugin_job_schedules (tenant_id, plugin_id, job_id, interval_seconds)
    SELECT installation.tenant_id, installation.plugin_id, p_job_id, p_interval_seconds
    FROM public.tenant_plugin_installations AS installation
    JOIN public.plugin_versions AS version
      ON version.plugin_id = installation.plugin_id
     AND version.version = installation.active_version
    WHERE installation.plugin_id = p_plugin_id
      AND installation.state = 'enabled'
      AND version.lifecycle_state = 'health_checked'
    ON CONFLICT (tenant_id, plugin_id, job_id) DO NOTHING;
    GET DIAGNOSTICS registered = ROW_COUNT;
    RETURN registered;
END;
$$;

-- Lease due schedules whose tenant installation is currently enabled and
-- health-checked. Disabled installations are never claimed, so their work is
-- paused rather than drained into a backlog.
CREATE FUNCTION platform_claim_plugin_jobs(
    p_plugin_id text,
    p_job_id text,
    p_batch_size bigint,
    p_claim_token uuid,
    p_lease_ms bigint
)
RETURNS TABLE (
    tenant_id uuid,
    checkpoint jsonb,
    run_count bigint,
    attempt_count integer,
    claim_expires_at timestamptz
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_plugin_id IS NULL
       OR p_job_id IS NULL
       OR p_batch_size IS NULL
       OR p_batch_size NOT BETWEEN 1 AND 1000
       OR p_claim_token IS NULL
       OR p_lease_ms IS NULL
       OR p_lease_ms NOT BETWEEN 1 AND 3600000 THEN
        RAISE EXCEPTION 'invalid plugin job claim input' USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    WITH candidates AS (
        SELECT schedule.tenant_id
        FROM public.plugin_job_schedules AS schedule
        JOIN public.tenant_plugin_installations AS installation
          ON installation.tenant_id = schedule.tenant_id
         AND installation.plugin_id = schedule.plugin_id
        JOIN public.plugin_versions AS version
          ON version.plugin_id = installation.plugin_id
         AND version.version = installation.active_version
        WHERE schedule.plugin_id = p_plugin_id
          AND schedule.job_id = p_job_id
          AND schedule.next_run_at <= now()
          AND (schedule.claim_expires_at IS NULL OR schedule.claim_expires_at <= now())
          AND installation.state = 'enabled'
          AND version.lifecycle_state = 'health_checked'
        ORDER BY schedule.next_run_at, schedule.tenant_id
        FOR UPDATE OF schedule SKIP LOCKED
        LIMIT p_batch_size
    )
    UPDATE public.plugin_job_schedules AS claimed
    SET claim_token = p_claim_token,
        claim_expires_at = now() + (p_lease_ms * INTERVAL '1 millisecond'),
        attempt_count = claimed.attempt_count + 1,
        last_started_at = now(),
        updated_at = now()
    FROM candidates
    WHERE claimed.tenant_id = candidates.tenant_id
      AND claimed.plugin_id = p_plugin_id
      AND claimed.job_id = p_job_id
    RETURNING claimed.tenant_id,
              claimed.checkpoint,
              claimed.run_count,
              claimed.attempt_count,
              claimed.claim_expires_at;
END;
$$;

-- Settle a successful run inside the job's own tenant transaction. The tenant
-- comes only from the transaction-local context, and an expired or replaced
-- lease returns false so the caller rolls back the job's side effects. The
-- lease is checked against the wall clock at settlement, not the transaction
-- start time, so a run that outlives its lease cannot commit.
CREATE FUNCTION platform_complete_plugin_job(
    p_plugin_id text,
    p_job_id text,
    p_claim_token uuid,
    p_checkpoint jsonb
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    context_tenant uuid := nullif(current_setting('app.tenant_id', true), '')::uuid;
    affected bigint;
BEGIN
    IF context_tenant IS NULL
       OR p_plugin_id IS NULL
       OR p_job_id IS NULL
       OR p_claim_token IS NULL
       OR p_checkpoint IS NULL
       OR jsonb_typeof(p_checkpoint) <> 'object' THEN
        RAISE EXCEPTION 'invalid plugin job completion input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.plugin_job_schedules AS schedule
    SET checkpoint = p_checkpoint,
        next_run_at = now() + (schedule.interval_seconds * INTERVAL '1 second'),
        claim_token = NULL,
        claim_expires_at = NULL,
        attempt_count = 0,
        run_count = schedule.run_count + 1,
        last_succeeded_at = now(),
        last_error_code = NULL,
        updated_at = now()
    WHERE schedule.tenant_id = context_tenant
      AND schedule.plugin_id = p_plugin_id
      AND schedule.job_id = p_job_id
      AND schedule.claim_token = p_claim_token
      AND schedule.claim_expires_at > clock_timestamp();
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

-- Record a failed run after its tenant transaction rolled back.
CREATE FUNCTION platform_fail_plugin_job(
    p_tenant_id uuid,
    p_plugin_id text,
    p_job_id text,
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
       OR p_plugin_id IS NULL
       OR p_job_id IS NULL
       OR p_claim_token IS NULL
       OR p_retry_ms IS NULL
       OR p_retry_ms NOT BETWEEN 1 AND 86400000
       OR p_error_code IS NULL
       OR p_error_code !~ '^[a-z][a-z0-9._-]{0,63}$' THEN
        RAISE EXCEPTION 'invalid plugin job failure input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.plugin_job_schedules
    SET next_run_at = now() + (p_retry_ms * INTERVAL '1 millisecond'),
        claim_token = NULL,
        claim_expires_at = NULL,
        last_failed_at = now(),
        last_error_code = p_error_code,
        updated_at = now()
    WHERE tenant_id = p_tenant_id
      AND plugin_id = p_plugin_id
      AND job_id = p_job_id
      AND claim_token = p_claim_token;
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

-- Return a claim without running it, for example when activation changed
-- between claim and execution or this worker links a different plugin
-- version. The run is deferred by p_retry_ms so a schedule this worker cannot
-- serve does not starve other tenants' due runs.
CREATE FUNCTION platform_release_plugin_job(
    p_tenant_id uuid,
    p_plugin_id text,
    p_job_id text,
    p_claim_token uuid,
    p_retry_ms bigint
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
       OR p_plugin_id IS NULL
       OR p_job_id IS NULL
       OR p_claim_token IS NULL
       OR p_retry_ms IS NULL
       OR p_retry_ms NOT BETWEEN 1 AND 86400000 THEN
        RAISE EXCEPTION 'invalid plugin job release input' USING ERRCODE = '22023';
    END IF;

    UPDATE public.plugin_job_schedules
    SET claim_token = NULL,
        claim_expires_at = NULL,
        attempt_count = greatest(attempt_count - 1, 0),
        next_run_at = greatest(next_run_at, now() + (p_retry_ms * INTERVAL '1 millisecond')),
        updated_at = now()
    WHERE tenant_id = p_tenant_id
      AND plugin_id = p_plugin_id
      AND job_id = p_job_id
      AND claim_token = p_claim_token;
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

REVOKE ALL ON FUNCTION platform_ensure_plugin_job_schedules(text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_claim_plugin_jobs(text, text, bigint, uuid, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_complete_plugin_job(text, text, uuid, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_fail_plugin_job(uuid, text, text, uuid, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION platform_release_plugin_job(uuid, text, text, uuid, bigint) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION platform_ensure_plugin_job_schedules(text, text, integer) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_claim_plugin_jobs(text, text, bigint, uuid, bigint) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_fail_plugin_job(uuid, text, text, uuid, bigint, text) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_release_plugin_job(uuid, text, text, uuid, bigint) TO platform_worker;
GRANT EXECUTE ON FUNCTION platform_complete_plugin_job(text, text, uuid, jsonb) TO platform_app;
