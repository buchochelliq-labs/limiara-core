-- The workspace metadata version (ADR 0024 §3, continual sync). Every
-- change to objects, fields, object permissions, validation rules or
-- automations increments it in the same transaction, so a device following
-- the change log learns that definitions moved before it applies records
-- made under them. change_counters is tenant-scoped with forced RLS.
ALTER TABLE change_counters
    ADD COLUMN metadata_version bigint NOT NULL DEFAULT 0 CHECK (metadata_version >= 0);
