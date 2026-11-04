-- Background and scheduled automations (ADR 0022 §6): jobs are queued in
-- the transaction of the change that fires them and run later by the
-- worker, each in its workspace's own transaction, with retries.
ALTER TABLE automations DROP CONSTRAINT automations_timing_check;
ALTER TABLE automations ADD CONSTRAINT automations_timing_check
    CHECK (timing IN ('before_save', 'after_save', 'async', 'scheduled'));
ALTER TABLE automations ADD COLUMN schedule jsonb
    CHECK (schedule IS NULL OR jsonb_typeof(schedule) = 'object');

CREATE TABLE automation_jobs (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    automation_id uuid NOT NULL,
    object_id uuid NOT NULL,
    record_id uuid NOT NULL,
    record text NOT NULL CHECK (char_length(record) BETWEEN 1 AND 40),
    event text NOT NULL CHECK (event IN ('create', 'update', 'delete', 'undelete')),
    scheduled boolean NOT NULL,
    due_at timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    locked_until timestamptz,
    last_error text CHECK (char_length(last_error) <= 2000),
    depth integer NOT NULL DEFAULT 0 CHECK (depth >= 0),
    actor_id uuid NOT NULL,
    correlation_id uuid NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, id)
);
-- One pending scheduled job per automation and record; a later change moves it.
CREATE UNIQUE INDEX automation_jobs_scheduled_idx ON automation_jobs (tenant_id, automation_id, record_id)
    WHERE scheduled AND status = 'pending';
CREATE INDEX automation_jobs_automation_idx ON automation_jobs (tenant_id, automation_id, created_at DESC);
CREATE INDEX automation_jobs_due_idx ON automation_jobs (due_at) WHERE status IN ('pending', 'running');

ALTER TABLE automation_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE automation_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY automation_jobs_tenant_isolation ON automation_jobs TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY automation_jobs_migrator_management ON automation_jobs TO platform_migrator
    USING (true) WITH CHECK (true);

-- Due jobs across workspaces, for the worker. Discovery is the only
-- cross-tenant step: each job then runs in its workspace's transaction.
-- Jobs whose worker stopped holding them come back once their lock passes.
CREATE FUNCTION platform_due_automation_jobs(p_limit integer)
RETURNS TABLE (tenant_id uuid, id uuid)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT j.tenant_id, j.id
  FROM automation_jobs j
  WHERE (j.status = 'pending' AND j.due_at <= now())
     OR (j.status = 'running' AND j.locked_until < now())
  ORDER BY j.due_at, j.tenant_id, j.id
  LIMIT least(greatest(p_limit, 1), 500)
$$;

REVOKE ALL ON FUNCTION platform_due_automation_jobs(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_due_automation_jobs(integer) TO platform_worker;
