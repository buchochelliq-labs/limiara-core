-- Close the remaining tenant-scoped kernel tables. Before this migration the
-- RBAC tables had no row-level security and platform_app held full DML on
-- them, so a single application bug could grant roles across tenants.
--
-- After it:
-- - every tenant-scoped kernel table forces RLS on tenant_id;
-- - platform_app is read-only on membership and role assignments, which are
--   provisioned by migrations or future bounded administration routines;
-- - the pre-tenant-context workspace lookup is a bounded SECURITY DEFINER
--   routine that answers only for a live session's own user.

ALTER TABLE memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE memberships FORCE ROW LEVEL SECURITY;
ALTER TABLE roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE roles FORCE ROW LEVEL SECURITY;
ALTER TABLE role_permissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE role_permissions FORCE ROW LEVEL SECURITY;
ALTER TABLE user_roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_roles FORCE ROW LEVEL SECURITY;
ALTER TABLE idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE idempotency_keys FORCE ROW LEVEL SECURITY;
ALTER TABLE entitlement_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE entitlement_snapshots FORCE ROW LEVEL SECURITY;
ALTER TABLE deployment_placements ENABLE ROW LEVEL SECURITY;
ALTER TABLE deployment_placements FORCE ROW LEVEL SECURITY;

CREATE POLICY memberships_tenant_isolation ON memberships TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY roles_tenant_isolation ON roles TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY role_permissions_tenant_isolation ON role_permissions TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY user_roles_tenant_isolation ON user_roles TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY idempotency_keys_tenant_isolation ON idempotency_keys TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY entitlement_snapshots_tenant_isolation ON entitlement_snapshots TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY deployment_placements_tenant_isolation ON deployment_placements TO platform_app
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

-- FORCE RLS also applies to the owner. Migrations seed these tables and the
-- bounded routine below reads them as the owner.
CREATE POLICY memberships_migrator_management ON memberships TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY roles_migrator_management ON roles TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY role_permissions_migrator_management ON role_permissions TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY user_roles_migrator_management ON user_roles TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY idempotency_keys_migrator_management ON idempotency_keys TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY entitlement_snapshots_migrator_management ON entitlement_snapshots TO platform_migrator USING (true) WITH CHECK (true);
CREATE POLICY deployment_placements_migrator_management ON deployment_placements TO platform_migrator USING (true) WITH CHECK (true);

-- Authorization data is not writable by the request role.
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON memberships, roles, role_permissions, user_roles FROM platform_app;
REVOKE TRUNCATE ON idempotency_keys, entitlement_snapshots, deployment_placements FROM platform_app;

-- Workspace access for the user of one live session, optionally narrowed to a
-- single workspace. Revoked, expired, or mismatched sessions return no rows,
-- so the routine cannot enumerate another user's memberships.
CREATE FUNCTION platform_session_workspace_access(
    p_session_id uuid,
    p_user_id uuid,
    p_tenant_id uuid
)
RETURNS TABLE (
    tenant_id uuid,
    display_name text,
    role_name text,
    permission_id text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.display_name, r.name, rp.permission_id
    FROM public.sessions AS s
    JOIN public.memberships AS m ON m.user_id = s.user_id
    JOIN public.tenants AS t ON t.id = m.tenant_id
    LEFT JOIN public.user_roles AS ur ON ur.tenant_id = m.tenant_id AND ur.user_id = m.user_id
    LEFT JOIN public.roles AS r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id
    LEFT JOIN public.role_permissions AS rp ON rp.tenant_id = r.tenant_id AND rp.role_id = r.id
    WHERE s.id = p_session_id
      AND s.user_id = p_user_id
      AND s.revoked_at IS NULL
      AND s.expires_at > now()
      AND (p_tenant_id IS NULL OR m.tenant_id = p_tenant_id)
    ORDER BY t.display_name, t.id, r.name, rp.permission_id
$$;

REVOKE ALL ON FUNCTION platform_session_workspace_access(uuid, uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_session_workspace_access(uuid, uuid, uuid) TO platform_app;
