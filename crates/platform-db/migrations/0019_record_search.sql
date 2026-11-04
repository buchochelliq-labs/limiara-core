-- Global search (ADR 0020, #305): an inverted index of record names and
-- searchable field values, written with the record's pivots in the same
-- transaction. Terms are byte-ordered so prefix ranges behave identically
-- on every backend and cluster locale.
CREATE TABLE record_search_terms (
    tenant_id uuid NOT NULL,
    term text COLLATE "C" NOT NULL CHECK (char_length(term) BETWEEN 1 AND 40),
    record_id uuid NOT NULL,
    weight smallint NOT NULL CHECK (weight BETWEEN 1 AND 3),
    PRIMARY KEY (tenant_id, term, record_id),
    FOREIGN KEY (tenant_id, record_id) REFERENCES records(tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX record_search_terms_record_idx ON record_search_terms (tenant_id, record_id);

ALTER TABLE record_search_terms ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_search_terms FORCE ROW LEVEL SECURITY;
CREATE POLICY record_search_terms_tenant_isolation ON record_search_terms TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY record_search_terms_migrator_management ON record_search_terms TO platform_migrator
    USING (true) WITH CHECK (true);
-- Terms are replaced, never edited.
REVOKE UPDATE ON record_search_terms FROM platform_app;
