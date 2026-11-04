-- Bulk jobs (ADR 0023 §2): rows applied in the background by the worker,
-- each as a change of its own with the submitting member's current access,
-- the row's result written in the same transaction as its change.
CREATE TABLE bulk_jobs (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    object text NOT NULL CHECK (char_length(object) BETWEEN 1 AND 128),
    operation text NOT NULL CHECK (operation IN ('insert', 'update', 'delete')),
    status text NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'aborted')),
    total bigint NOT NULL CHECK (total >= 0),
    succeeded bigint NOT NULL DEFAULT 0 CHECK (succeeded >= 0),
    failed bigint NOT NULL DEFAULT 0 CHECK (failed >= 0),
    actor_id uuid NOT NULL,
    correlation_id uuid NOT NULL,
    locked_until timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz,
    PRIMARY KEY (tenant_id, id)
);
-- Jobs are taken least recently touched first: a job handed back after a
-- batch goes behind the jobs waiting, so every job gets a turn.
CREATE INDEX bulk_jobs_due_idx ON bulk_jobs (updated_at) WHERE status IN ('queued', 'running');

CREATE TABLE bulk_job_rows (
    tenant_id uuid NOT NULL,
    job_id uuid NOT NULL,
    row_index integer NOT NULL CHECK (row_index >= 0),
    input jsonb NOT NULL CHECK (jsonb_typeof(input) = 'object'),
    status text NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
    record text,
    errors jsonb,
    PRIMARY KEY (tenant_id, job_id, row_index),
    FOREIGN KEY (tenant_id, job_id) REFERENCES bulk_jobs (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX bulk_job_rows_pending_idx ON bulk_job_rows (tenant_id, job_id, row_index) WHERE status = 'pending';

ALTER TABLE bulk_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE bulk_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY bulk_jobs_tenant_isolation ON bulk_jobs TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY bulk_jobs_migrator_management ON bulk_jobs TO platform_migrator
    USING (true) WITH CHECK (true);
ALTER TABLE bulk_job_rows ENABLE ROW LEVEL SECURITY;
ALTER TABLE bulk_job_rows FORCE ROW LEVEL SECURITY;
CREATE POLICY bulk_job_rows_tenant_isolation ON bulk_job_rows TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY bulk_job_rows_migrator_management ON bulk_job_rows TO platform_migrator
    USING (true) WITH CHECK (true);

-- Bulk jobs with work to do, across workspaces, for the worker. A job
-- whose worker stopped holding it comes back once its lock passes.
CREATE FUNCTION platform_due_bulk_jobs(p_limit integer)
RETURNS TABLE (tenant_id uuid, id uuid)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT j.tenant_id, j.id
  FROM bulk_jobs j
  WHERE j.status = 'queued' OR (j.status = 'running' AND j.locked_until < now())
  ORDER BY j.updated_at, j.tenant_id, j.id
  LIMIT least(greatest(p_limit, 1), 100)
$$;

REVOKE ALL ON FUNCTION platform_due_bulk_jobs(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_due_bulk_jobs(integer) TO platform_worker;
