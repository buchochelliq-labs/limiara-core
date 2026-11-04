-- API clients (ADR 0023 §3): OAuth 2.0 client-credential clients that let
-- an integration call the object engine as one workspace member, without a
-- person signing in. Secrets and access tokens are high-entropy random
-- values stored only as SHA-256 hashes. There are no passwords.
CREATE TABLE api_clients (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    client_id text NOT NULL UNIQUE CHECK (char_length(client_id) BETWEEN 16 AND 64),
    secret_hash text NOT NULL CHECK (char_length(secret_hash) = 64),
    run_as uuid NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamptz NOT NULL,
    last_used_at timestamptz,
    revoked_at timestamptz,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE api_client_tokens (
    tenant_id uuid NOT NULL,
    token_hash text NOT NULL UNIQUE CHECK (char_length(token_hash) = 64),
    client uuid NOT NULL,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, token_hash),
    FOREIGN KEY (tenant_id, client) REFERENCES api_clients (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX api_client_tokens_client_idx ON api_client_tokens (tenant_id, client, expires_at);

ALTER TABLE api_clients ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_clients FORCE ROW LEVEL SECURITY;
CREATE POLICY api_clients_tenant_isolation ON api_clients TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY api_clients_migrator_management ON api_clients TO platform_migrator
    USING (true) WITH CHECK (true);
ALTER TABLE api_client_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_client_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY api_client_tokens_tenant_isolation ON api_client_tokens TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY api_client_tokens_migrator_management ON api_client_tokens TO platform_migrator
    USING (true) WITH CHECK (true);

INSERT INTO permissions (id, description) VALUES
  ('api_clients.manage', 'Create and revoke API clients that act as workspace members')
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT r.tenant_id, r.id, 'api_clients.manage'
FROM roles r
WHERE r.tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND r.id = '018f0000-0000-7000-8000-000000000201'
ON CONFLICT DO NOTHING;

-- Exchanges a client's credentials for an access token. The token
-- endpoint runs before any workspace is known, so this bounded routine is
-- the only way to find a client by its public id. It answers only when
-- the secret matches, the client is not revoked and the member it acts as
-- still belongs to the workspace; otherwise it returns no row and writes
-- nothing. Each issue is audited in the client's workspace.
CREATE FUNCTION platform_issue_api_token(
    p_client_id text,
    p_secret_hash text,
    p_token_hash text,
    p_expires_at timestamptz,
    p_correlation_id uuid
)
RETURNS TABLE (tenant_id uuid, client uuid, run_as uuid)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    matched public.api_clients%ROWTYPE;
BEGIN
    SELECT c.* INTO matched
    FROM public.api_clients AS c
    JOIN public.memberships AS m ON m.tenant_id = c.tenant_id AND m.user_id = c.run_as
    WHERE c.client_id = p_client_id
      AND c.secret_hash = p_secret_hash
      AND c.revoked_at IS NULL;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    DELETE FROM public.api_client_tokens AS t
    WHERE t.tenant_id = matched.tenant_id AND t.client = matched.id AND t.expires_at < now();
    INSERT INTO public.api_client_tokens (tenant_id, token_hash, client, created_at, expires_at)
    VALUES (matched.tenant_id, p_token_hash, matched.id, now(), p_expires_at);
    UPDATE public.api_clients AS c SET last_used_at = now()
    WHERE c.tenant_id = matched.tenant_id AND c.id = matched.id;
    INSERT INTO public.audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata)
    VALUES (gen_random_uuid(), matched.tenant_id, matched.run_as, p_correlation_id,
            'api_clients.token.issued', 'api_client:' || matched.id::text,
            jsonb_build_object('expires_at', p_expires_at));
    RETURN QUERY SELECT matched.tenant_id, matched.id, matched.run_as;
END
$$;

-- Workspace access for the member an unexpired token of an unrevoked
-- client acts as, in one workspace. Mirrors the session routine: no rows
-- for anything else, so it cannot enumerate other workspaces.
CREATE FUNCTION platform_api_token_access(p_token_hash text, p_tenant_id uuid)
RETURNS TABLE (
    tenant_id uuid,
    display_name text,
    user_id uuid,
    client uuid,
    role_name text,
    permission_id text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.display_name, c.run_as, c.id, r.name, rp.permission_id
    FROM public.api_client_tokens AS k
    JOIN public.api_clients AS c ON c.tenant_id = k.tenant_id AND c.id = k.client
    JOIN public.memberships AS m ON m.tenant_id = c.tenant_id AND m.user_id = c.run_as
    JOIN public.tenants AS t ON t.id = m.tenant_id
    LEFT JOIN public.user_roles AS ur ON ur.tenant_id = m.tenant_id AND ur.user_id = m.user_id
    LEFT JOIN public.roles AS r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id
    LEFT JOIN public.role_permissions AS rp ON rp.tenant_id = r.tenant_id AND rp.role_id = r.id
    WHERE k.token_hash = p_token_hash
      AND k.tenant_id = p_tenant_id
      AND k.expires_at > now()
      AND c.revoked_at IS NULL
    ORDER BY r.name, rp.permission_id
$$;

REVOKE ALL ON FUNCTION platform_issue_api_token(text, text, text, timestamptz, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_issue_api_token(text, text, text, timestamptz, uuid) TO platform_app;
REVOKE ALL ON FUNCTION platform_api_token_access(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_api_token_access(text, uuid) TO platform_app;
