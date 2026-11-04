-- Standard record fields (ADR 0014).
--
-- created_by and updated_by hold the kernel user who created and last
-- changed the record; rows written before this migration have no creator.
-- touched_at moves on every change, including system changes such as
-- retention purges; existing rows start at the time of this migration, which
-- is itself a change to every record.
ALTER TABLE p_contacts.people
  ADD COLUMN created_by uuid,
  ADD COLUMN updated_by uuid,
  ADD COLUMN touched_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE p_contacts.organisations
  ADD COLUMN created_by uuid,
  ADD COLUMN updated_by uuid,
  ADD COLUMN touched_at timestamptz NOT NULL DEFAULT now();

CREATE INDEX people_tenant_touched_idx ON p_contacts.people (tenant_id, touched_at, id);
CREATE INDEX organisations_tenant_touched_idx ON p_contacts.organisations (tenant_id, touched_at, id);
