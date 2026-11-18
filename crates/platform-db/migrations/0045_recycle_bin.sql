-- The engine's recycle bin (ADR 0035 G2 and G3).
--
-- G2: a per-workspace purge removes records that have been in the recycle
-- bin for 15 days. Deleting a record row cascades to its pivots, search
-- terms, shares and history, as the comments of 0015 and 0017 planned; the
-- purge removes the record's change-log rows itself first, under the policy
-- below. Audit entries and published events are evidence and stay.
--
-- G3: deleting a record clears the lookups that point to it on other
-- records. Each cleared value is kept here until its target is undeleted
-- (and the value restored where the field was not changed since) or
-- purged.

-- The purge finds a workspace's oldest recycle-bin records first; a
-- partial, tenant-first index keeps that cheap however large the
-- workspace is.
CREATE INDEX records_tenant_bin_idx ON records (tenant_id, deleted_at, id)
    WHERE deleted_at IS NOT NULL;

-- A record in the recycle bin may lose its change-log rows before the
-- 72-hour window ends: the purge removes them with the record. Rows of live
-- records stay append-only, as 0018 set out.
CREATE POLICY record_changes_purged_removal ON record_changes FOR DELETE TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
           AND EXISTS (
               SELECT 1 FROM records r
               WHERE r.tenant_id = record_changes.tenant_id
                 AND r.id = record_changes.record_id
                 AND r.deleted_at IS NOT NULL));

-- How many purge passes removed records, carried on change pages so a
-- synced device asks which of the records it holds are gone.
ALTER TABLE change_counters ADD COLUMN purges bigint NOT NULL DEFAULT 0 CHECK (purges >= 0);

CREATE TABLE record_cleared_lookups (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    -- The record whose deletion cleared the value.
    deleted_record_id uuid NOT NULL,
    -- The referencing record and its reference field.
    record_id uuid NOT NULL,
    field_id uuid NOT NULL,
    -- The value cleared: the target's public id.
    value jsonb NOT NULL CHECK (octet_length(value::text) <= 1024),
    -- The referencing record's version just after the clear.
    cleared_version bigint NOT NULL CHECK (cleared_version >= 1),
    -- Whether the field was written since; a changed field is left alone.
    changed boolean NOT NULL DEFAULT false,
    cleared_at timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, deleted_record_id, record_id, field_id),
    FOREIGN KEY (tenant_id, deleted_record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, field_id) REFERENCES field_definitions(tenant_id, id)
);
-- Writes to a referencing record's field mark its kept values changed.
CREATE INDEX record_cleared_lookups_record_idx ON record_cleared_lookups (tenant_id, record_id, field_id);

ALTER TABLE record_cleared_lookups ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_cleared_lookups FORCE ROW LEVEL SECURITY;
CREATE POLICY record_cleared_lookups_tenant_isolation ON record_cleared_lookups TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_cleared_lookups_migrator_management ON record_cleared_lookups TO platform_migrator
    USING (true) WITH CHECK (true);

-- Workspaces with records in the recycle bin since before p_before, for
-- the worker. Discovery is the only cross-tenant step: each workspace is
-- then purged in its own transactions, under its row-level security.
CREATE FUNCTION platform_due_recycle_bin_purges(p_before timestamptz, p_limit integer)
RETURNS TABLE (tenant_id uuid)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT t.id
  FROM tenants t
  WHERE EXISTS (
      SELECT 1 FROM records r
      WHERE r.tenant_id = t.id AND r.deleted_at IS NOT NULL AND r.deleted_at < p_before)
  ORDER BY t.id
  LIMIT least(greatest(p_limit, 1), 500)
$$;

REVOKE ALL ON FUNCTION platform_due_recycle_bin_purges(timestamptz, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_due_recycle_bin_purges(timestamptz, integer) TO platform_worker;
