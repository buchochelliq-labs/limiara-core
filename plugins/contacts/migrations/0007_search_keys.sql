-- Search and sort keys computed by the Contacts service (ADR 0013).
--
-- lower() follows the database locale, so a cluster initialised with the C
-- locale neither matches "ÉMILE" to "Émile" nor orders names the way list
-- cursors expect. The service now stores Unicode-lowercased keys it computes
-- itself, identically on every storage backend. Rows written before this
-- migration have no key until their next update and fall back to lower();
-- forced RLS keeps this migration from backfilling them in SQL.
ALTER TABLE p_contacts.people
  ADD COLUMN name_key text,
  ADD COLUMN email_key text;
ALTER TABLE p_contacts.organisations
  ADD COLUMN name_key text,
  ADD COLUMN email_key text;

CREATE INDEX people_tenant_name_key_idx
  ON p_contacts.people (tenant_id, (coalesce(name_key, lower(full_name))), id);
CREATE INDEX people_tenant_email_key_idx
  ON p_contacts.people (tenant_id, (coalesce(email_key, lower(email))))
  WHERE email IS NOT NULL;
CREATE INDEX organisations_tenant_name_key_idx
  ON p_contacts.organisations (tenant_id, (coalesce(name_key, lower(name))), id);
CREATE INDEX organisations_tenant_email_key_idx
  ON p_contacts.organisations (tenant_id, (coalesce(email_key, lower(email))))
  WHERE email IS NOT NULL;
