-- Values that other plugins add to Contacts records (ADR 0012).
--
-- Each column holds one object per extending plugin, keyed by plugin id and
-- then by field key. Contacts never interprets the values: it stores only
-- patches the kernel extension service has validated, inside the Contacts
-- transaction, so the existing tenant and plugin-context RLS policies, record
-- versions, audit and events cover them like any other column.
ALTER TABLE p_contacts.people
  ADD COLUMN extensions jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD CONSTRAINT people_extensions_object
    CHECK (jsonb_typeof(extensions) = 'object' AND pg_column_size(extensions) <= 65536);
ALTER TABLE p_contacts.organisations
  ADD COLUMN extensions jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD CONSTRAINT organisations_extensions_object
    CHECK (jsonb_typeof(extensions) = 'object' AND pg_column_size(extensions) <= 65536);

-- Containment lookups for list filters; every query also constrains tenant_id.
CREATE INDEX people_extensions_gin_idx
  ON p_contacts.people USING gin (extensions jsonb_path_ops);
CREATE INDEX organisations_extensions_gin_idx
  ON p_contacts.organisations USING gin (extensions jsonb_path_ops);
