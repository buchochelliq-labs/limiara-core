-- Saved reports (ADR 0027, #313): a record query with a name and a way to
-- show its rows. Reports are run as whoever opens them, so the table holds
-- definitions only, never results.
CREATE TABLE reports (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    description text CHECK (description IS NULL OR char_length(description) <= 1000),
    query text NOT NULL CHECK (char_length(query) BETWEEN 1 AND 10000),
    chart text NOT NULL CHECK (chart IN ('table', 'bar', 'line', 'pie', 'metric')),
    label_column text CHECK (label_column IS NULL OR char_length(label_column) <= 200),
    value_column text CHECK (value_column IS NULL OR char_length(value_column) <= 200),
    visibility text NOT NULL CHECK (visibility IN ('private', 'workspace')),
    owner_id uuid NOT NULL,
    version bigint NOT NULL CHECK (version >= 1),
    created_at timestamptz NOT NULL,
    created_by uuid,
    updated_at timestamptz NOT NULL,
    updated_by uuid,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX reports_visibility_idx ON reports (tenant_id, visibility, owner_id);

ALTER TABLE reports ENABLE ROW LEVEL SECURITY;
ALTER TABLE reports FORCE ROW LEVEL SECURITY;
CREATE POLICY reports_tenant_isolation ON reports TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY reports_migrator_management ON reports TO platform_migrator
    USING (true) WITH CHECK (true);

INSERT INTO permissions (id, description) VALUES
  ('reports.manage', 'Share reports with the workspace and change or delete anyone''s')
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT r.tenant_id, r.id, 'reports.manage'
FROM roles r
WHERE r.tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND r.id = '018f0000-0000-7000-8000-000000000201'
ON CONFLICT DO NOTHING;
