-- Plugin removal with a retention window (ADR 0012, #277).
--
-- Removing a plugin keeps its installation row in the `removed` state. While
-- removed, the plugin is inactive exactly as when disabled, and the values it
-- stored with other plugins' records stay in place so an administrator can
-- export them or reinstall. After the retention window the worker asks each
-- host to purge the plugin's namespace and records `extensions_purged_at`.
ALTER TABLE tenant_plugin_installations
  ADD COLUMN removed_at timestamptz,
  ADD COLUMN extensions_purged_at timestamptz,
  ADD CONSTRAINT tenant_plugin_installations_removed_state
    CHECK (removed_at IS NULL OR state = 'removed'),
  ADD CONSTRAINT tenant_plugin_installations_purge_after_removal
    CHECK (extensions_purged_at IS NULL OR removed_at IS NOT NULL);

CREATE INDEX tenant_plugin_removed_due_idx
  ON tenant_plugin_installations (removed_at)
  WHERE state = 'removed' AND extensions_purged_at IS NULL;

-- Removed installations whose retention window has passed and whose
-- extension values have not been purged yet, across tenants. The worker uses
-- only this bounded routine for cross-tenant discovery; the purge itself and
-- its evidence run in each tenant's own transaction.
CREATE FUNCTION platform_due_extension_purges(p_retention_days integer, p_limit integer)
RETURNS TABLE (tenant_id uuid, plugin_id text, manifest jsonb)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT i.tenant_id, i.plugin_id, v.manifest
  FROM tenant_plugin_installations i
  JOIN plugin_versions v
    ON v.plugin_id = i.plugin_id AND v.version = i.active_version
  WHERE i.state = 'removed'
    AND i.extensions_purged_at IS NULL
    AND i.removed_at <= now() - make_interval(days => greatest(p_retention_days, 1))
  ORDER BY i.removed_at, i.tenant_id, i.plugin_id
  LIMIT least(greatest(p_limit, 1), 500)
$$;

REVOKE ALL ON FUNCTION platform_due_extension_purges(integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_due_extension_purges(integer, integer) TO platform_worker;
