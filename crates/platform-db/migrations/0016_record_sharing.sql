-- Record ownership and sharing (ADR 0016, #286).

-- The reporting line: managers see and edit what their reports own on
-- objects that grant access using hierarchies. It is its own table because
-- memberships stay read-only to the application role (migration 0011);
-- changing a line needs members.manage and is audited. Both ends are
-- members of the workspace, a member never reports to themself, and
-- leaving the workspace removes the lines on either side.
CREATE TABLE reporting_lines (
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    manager_user_id uuid NOT NULL,
    updated_at timestamptz NOT NULL,
    updated_by uuid,
    PRIMARY KEY (tenant_id, user_id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES memberships(tenant_id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, manager_user_id) REFERENCES memberships(tenant_id, user_id) ON DELETE CASCADE,
    CHECK (manager_user_id <> user_id)
);
CREATE INDEX reporting_lines_manager_idx ON reporting_lines (tenant_id, manager_user_id, user_id);

-- Whether the hierarchy grants access to an object's records (Salesforce
-- "Grant access using hierarchies"; on by default).
ALTER TABLE object_definitions ADD COLUMN grant_hierarchy boolean NOT NULL DEFAULT true;

-- Manual shares: one record, one member or role, read or edit.
CREATE TABLE record_shares (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    record_id uuid NOT NULL,
    grantee_kind text NOT NULL CHECK (grantee_kind IN ('user', 'role')),
    grantee_id uuid NOT NULL,
    access text NOT NULL CHECK (access IN ('read', 'edit')),
    reason text NOT NULL DEFAULT 'manual' CHECK (reason IN ('manual')),
    created_at timestamptz NOT NULL,
    created_by uuid,
    PRIMARY KEY (tenant_id, record_id, grantee_kind, grantee_id),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX record_shares_grantee_idx ON record_shares (tenant_id, grantee_kind, grantee_id, record_id);

DO $$
DECLARE
    table_name text;
BEGIN
    FOREACH table_name IN ARRAY ARRAY['reporting_lines', 'record_shares'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format(
            'CREATE POLICY %I ON %I TO platform_app '
            'USING (tenant_id = nullif(current_setting(''app.tenant_id'', true), '''')::uuid) '
            'WITH CHECK (tenant_id = nullif(current_setting(''app.tenant_id'', true), '''')::uuid)',
            table_name || '_tenant_isolation', table_name);
        EXECUTE format(
            'CREATE POLICY %I ON %I TO platform_migrator USING (true) WITH CHECK (true)',
            table_name || '_migrator_management', table_name);
    END LOOP;
END
$$;

-- Moving records someone else owns (Salesforce "Transfer Record"), and
-- managing reporting lines.
INSERT INTO permissions (id, description) VALUES
  ('records.transfer', 'Change the owner of records someone else owns'),
  ('members.manage', 'Set who workspace members report to')
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT r.tenant_id, r.id, p.id
FROM roles r
CROSS JOIN permissions p
WHERE r.tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND r.id = '018f0000-0000-7000-8000-000000000201'
  AND p.id IN ('records.transfer', 'members.manage')
ON CONFLICT DO NOTHING;
