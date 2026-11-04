CREATE TABLE ui_external_publications (
    slug_digest text PRIMARY KEY CHECK (slug_digest ~ '^[0-9a-f]{64}$'),
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    host text NOT NULL CHECK (host IN ('public-form', 'portal')),
    generation bigint NOT NULL CHECK (generation > 0),
    portal_audience text,
    compiled jsonb NOT NULL,
    submission_policy jsonb,
    enabled_plugins text[] NOT NULL DEFAULT '{}',
    revoked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (compiled->>'host' = host),
    CHECK ((host = 'portal') = (portal_audience IS NOT NULL)),
    CHECK (portal_audience IS NULL OR portal_audience <> ''),
    CHECK ((host = 'public-form') = (submission_policy IS NOT NULL))
);

ALTER TABLE ui_external_publications ENABLE ROW LEVEL SECURITY;
ALTER TABLE ui_external_publications FORCE ROW LEVEL SECURITY;

CREATE POLICY ui_external_publications_tenant_isolation
ON ui_external_publications
USING (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    OR slug_digest = nullif(current_setting('app.external_slug_digest', true), '')
)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);

CREATE INDEX ui_external_publications_tenant_generation_idx
ON ui_external_publications (tenant_id, host, revoked, generation DESC);

-- Public routes do not know a tenant until an opaque slug has been resolved.
-- This deliberately supports only an exact one-way digest lookup and exposes no
-- listing/search surface. Deployments grant EXECUTE only to the API runtime role.
CREATE FUNCTION lookup_ui_external_publication(p_slug_digest text)
RETURNS TABLE (
    tenant_id uuid,
    portal_audience text,
    compiled jsonb,
    generation bigint,
    enabled_plugins text[],
    revoked boolean,
    submission_policy jsonb
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_slug_digest !~ '^[0-9a-f]{64}$' THEN
        RETURN;
    END IF;
    PERFORM pg_catalog.set_config('app.external_slug_digest', p_slug_digest, true);
    RETURN QUERY
        SELECT publication.tenant_id,
               publication.portal_audience,
               publication.compiled,
               publication.generation,
               publication.enabled_plugins,
               publication.revoked,
               publication.submission_policy
        FROM public.ui_external_publications AS publication
        WHERE publication.slug_digest = p_slug_digest
        LIMIT 1;
END;
$$;

REVOKE ALL ON TABLE ui_external_publications FROM PUBLIC;
REVOKE ALL ON FUNCTION lookup_ui_external_publication(text) FROM PUBLIC;
