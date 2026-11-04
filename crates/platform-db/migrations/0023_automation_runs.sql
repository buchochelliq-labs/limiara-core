-- Automation run history (ADR 0022 §5): one row each time an automation's
-- condition held and it acted, including runs whose change was rolled
-- back. Kept for 30 days.
CREATE TABLE automation_runs (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    automation_id uuid NOT NULL,
    object_id uuid NOT NULL,
    record text NOT NULL CHECK (char_length(record) BETWEEN 1 AND 40),
    event text NOT NULL CHECK (event IN ('create', 'update', 'delete', 'undelete')),
    outcome text NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
    detail text CHECK (char_length(detail) <= 2000),
    saves bigint NOT NULL DEFAULT 0 CHECK (saves >= 0),
    ran_at timestamptz NOT NULL,
    ran_by uuid,
    correlation_id uuid,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX automation_runs_automation_idx ON automation_runs (tenant_id, automation_id, ran_at DESC, id DESC);
CREATE INDEX automation_runs_ran_at_idx ON automation_runs (tenant_id, ran_at);

ALTER TABLE automation_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE automation_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY automation_runs_tenant_isolation ON automation_runs TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY automation_runs_migrator_management ON automation_runs TO platform_migrator
    USING (true) WITH CHECK (true);

-- Automations on delete and restore.
ALTER TABLE automations DROP CONSTRAINT automations_trigger_check;
ALTER TABLE automations ADD CONSTRAINT automations_trigger_check
    CHECK (trigger IN ('create', 'update', 'create_or_update', 'delete', 'undelete'));
