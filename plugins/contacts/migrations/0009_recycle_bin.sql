-- Recycle bin (ADR 0014, issue #290).
--
-- deleted_at marks a record as deleted: hidden from reads, lists and writes,
-- restorable until the worker purges it after the retention window. The
-- standard is_deleted field is derived from it, so the two never disagree.
ALTER TABLE p_contacts.people ADD COLUMN deleted_at timestamptz;
ALTER TABLE p_contacts.organisations ADD COLUMN deleted_at timestamptz;

CREATE INDEX people_tenant_deleted_idx
  ON p_contacts.people (tenant_id, deleted_at, id) WHERE deleted_at IS NOT NULL;
CREATE INDEX organisations_tenant_deleted_idx
  ON p_contacts.organisations (tenant_id, deleted_at, id) WHERE deleted_at IS NOT NULL;
