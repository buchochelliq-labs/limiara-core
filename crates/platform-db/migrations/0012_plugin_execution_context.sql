-- Plugin execution context: separate plugins from each other inside the
-- shared RLS-bound runtime role without per-plugin database roles.
--
-- The host sets app.plugin_id transaction-locally (platform_db::begin_plugin_tx)
-- next to app.tenant_id before it runs plugin code. RESTRICTIVE policies are
-- ANDed with the existing tenant policies, so plugin work must satisfy both:
-- the right tenant and the right plugin. Code running without a plugin context
-- cannot use plugin-scoped rows at all.
--
-- This guards against bugs and confused deputies: a Contacts code path cannot
-- read or write another plugin's schedules or ledger entries, and no other
-- plugin context can touch Contacts tables. It is not a sandbox against
-- hostile in-process native code, which could set the variable itself; that
-- boundary belongs to the Wasm and external-service runtimes.

-- Job schedules and checkpoints are private plugin state.
CREATE POLICY plugin_job_schedules_plugin_context
ON plugin_job_schedules
AS RESTRICTIVE
TO platform_app
USING (plugin_id = nullif(current_setting('app.plugin_id', true), ''))
WITH CHECK (plugin_id = nullif(current_setting('app.plugin_id', true), ''));

-- Inbox markers may only be written under the current plugin's consumer
-- namespace. Reads stay tenant-scoped so operators and kernel diagnostics can
-- observe processing ledgers.
CREATE POLICY inbox_plugin_context_write
ON inbox
AS RESTRICTIVE
FOR INSERT
TO platform_app
WITH CHECK (
  left(consumer_id, octet_length(nullif(current_setting('app.plugin_id', true), '')) + 1)
    = nullif(current_setting('app.plugin_id', true), '') || '.'
);

-- A plugin can only publish events it owns: the envelope source and the event
-- type namespace must both be the current plugin.
CREATE POLICY outbox_plugin_context_write
ON outbox
AS RESTRICTIVE
FOR INSERT
TO platform_app
WITH CHECK (
  envelope->>'source_plugin' = nullif(current_setting('app.plugin_id', true), '')
  AND left(event_type, octet_length(nullif(current_setting('app.plugin_id', true), '')) + 1)
    = nullif(current_setting('app.plugin_id', true), '') || '.'
);

-- Job completion settles only the current plugin's schedule.
CREATE OR REPLACE FUNCTION platform_complete_plugin_job(
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
    context_plugin text := nullif(current_setting('app.plugin_id', true), '');
    affected bigint;
BEGIN
    IF context_tenant IS NULL
       OR context_plugin IS NULL
       OR p_plugin_id IS DISTINCT FROM context_plugin
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
      AND schedule.plugin_id = context_plugin
      AND schedule.job_id = p_job_id
      AND schedule.claim_token = p_claim_token
      AND schedule.claim_expires_at > clock_timestamp();
    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected = 1;
END;
$$;

REVOKE ALL ON FUNCTION platform_complete_plugin_job(text, text, uuid, jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_complete_plugin_job(text, text, uuid, jsonb) TO platform_app;
