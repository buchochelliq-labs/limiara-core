-- Cursor hashes are opaque caller capabilities, but global uniqueness still
-- leaks a known foreign hash through duplicate-key errors. Repository lookups
-- already include tenant_id, so make the key match that access boundary.
ALTER TABLE p_contacts.list_cursors DROP CONSTRAINT list_cursors_pkey;
ALTER TABLE p_contacts.list_cursors
  ADD PRIMARY KEY (tenant_id, token_hash);
