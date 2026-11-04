-- Device sign-in and revocation (ADR 0029, ADR 0024 §4). A phone or desktop
-- app signs a member in through the system browser with OIDC (RFC 8252):
-- the server runs the authorization code flow with PKCE against the
-- identity provider as it does for the web, then hands the app a one-time
-- code through its registered redirect URI. The app proves it started the
-- sign-in with its own PKCE verifier and receives a device registered in
-- one workspace, a short-lived access token and a rotating refresh token.
-- Codes and tokens are random and stored only as SHA-256 hashes. There are
-- no passwords.

-- A sign-in started by a device app carries where to return and the app's
-- own PKCE challenge. Browser sign-ins leave these empty.
ALTER TABLE oidc_login_transactions
    ADD COLUMN device_redirect_uri text CHECK (char_length(device_redirect_uri) BETWEEN 1 AND 512),
    ADD COLUMN device_code_challenge text CHECK (char_length(device_code_challenge) = 43),
    ADD COLUMN device_state text CHECK (char_length(device_state) BETWEEN 1 AND 512),
    ADD CONSTRAINT oidc_login_transactions_device_complete CHECK (
        (device_redirect_uri IS NULL) = (device_code_challenge IS NULL)
        AND (device_redirect_uri IS NULL) = (device_state IS NULL)
    );

-- One-time codes for a signed-in person, redeemed once by the app that
-- holds the matching PKCE verifier. Like sessions, they precede any
-- workspace.
CREATE TABLE device_authorizations (
    code_hash text PRIMARY KEY CHECK (char_length(code_hash) = 64),
    user_id uuid NOT NULL REFERENCES users(id),
    code_challenge text NOT NULL CHECK (char_length(code_challenge) = 43),
    redirect_uri text NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX device_authorizations_expiry_idx ON device_authorizations (expires_at);

CREATE TABLE devices (
    tenant_id uuid NOT NULL REFERENCES tenants(id),
    id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    platform text NOT NULL CHECK (platform IN ('ios', 'android', 'desktop')),
    -- Where content-free "workspace changed" notifications go.
    push_provider text CHECK (push_provider IN ('apns', 'fcm')),
    push_token text CHECK (char_length(push_token) BETWEEN 1 AND 4096),
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    revoked_at timestamptz,
    revoked_by uuid,
    revoked_reason text CHECK (revoked_reason IN ('revoked', 'signed_out', 'refresh_reused')),
    PRIMARY KEY (tenant_id, id),
    CHECK ((push_provider IS NULL) = (push_token IS NULL)),
    CHECK ((revoked_at IS NULL) = (revoked_reason IS NULL))
);
CREATE INDEX devices_user_idx ON devices (tenant_id, user_id, created_at DESC);

-- Tokens outlive their device's revocation until they expire, so a revoked
-- device presenting one is told to wipe its replica instead of getting a
-- plain 401.
CREATE TABLE device_tokens (
    tenant_id uuid NOT NULL,
    token_hash text NOT NULL UNIQUE CHECK (char_length(token_hash) = 64),
    device uuid NOT NULL,
    kind text NOT NULL CHECK (kind IN ('access', 'refresh')),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    -- When a refresh token was exchanged. Presenting it again means it was
    -- copied, so the device is revoked.
    used_at timestamptz,
    PRIMARY KEY (tenant_id, token_hash),
    FOREIGN KEY (tenant_id, device) REFERENCES devices (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX device_tokens_device_idx ON device_tokens (tenant_id, device, expires_at);

ALTER TABLE devices ENABLE ROW LEVEL SECURITY;
ALTER TABLE devices FORCE ROW LEVEL SECURITY;
CREATE POLICY devices_tenant_isolation ON devices TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY devices_migrator_management ON devices TO platform_migrator
    USING (true) WITH CHECK (true);
ALTER TABLE device_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE device_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY device_tokens_tenant_isolation ON device_tokens TO platform_app
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
CREATE POLICY device_tokens_migrator_management ON device_tokens TO platform_migrator
    USING (true) WITH CHECK (true);

INSERT INTO permissions (id, description) VALUES
  ('devices.manage', 'See and revoke every member''s devices')
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (tenant_id, role_id, permission_id)
SELECT r.tenant_id, r.id, 'devices.manage'
FROM roles r
WHERE r.tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND r.id = '018f0000-0000-7000-8000-000000000201'
ON CONFLICT DO NOTHING;

-- Exchanges a refresh token for a new access and refresh token. The token
-- endpoint runs before any workspace is known, so this bounded routine is
-- the only way to find a device by its refresh token. It returns:
-- - 'issued' after rotating the token of a device still in use;
-- - 'wipe' for a revoked device, a member who left, or a refresh token
--   presented a second time, which also revokes the device;
-- - no row for unknown or expired tokens.
-- Each outcome that changes a device is audited in its workspace.
CREATE FUNCTION platform_refresh_device_token(
    p_refresh_hash text,
    p_access_hash text,
    p_new_refresh_hash text,
    p_access_expires_at timestamptz,
    p_refresh_expires_at timestamptz,
    p_correlation_id uuid
)
RETURNS TABLE (tenant_id uuid, device uuid, user_id uuid, outcome text)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    token public.device_tokens%ROWTYPE;
    matched public.devices%ROWTYPE;
    member boolean;
BEGIN
    SELECT k.* INTO token
    FROM public.device_tokens AS k
    WHERE k.token_hash = p_refresh_hash AND k.kind = 'refresh' AND k.expires_at > now()
    FOR UPDATE;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    SELECT d.* INTO matched
    FROM public.devices AS d
    WHERE d.tenant_id = token.tenant_id AND d.id = token.device
    FOR UPDATE;
    SELECT EXISTS (
        SELECT 1 FROM public.memberships AS m
        WHERE m.tenant_id = matched.tenant_id AND m.user_id = matched.user_id
    ) INTO member;
    IF matched.revoked_at IS NOT NULL OR NOT member THEN
        RETURN QUERY SELECT matched.tenant_id, matched.id, matched.user_id, 'wipe'::text;
        RETURN;
    END IF;
    IF token.used_at IS NOT NULL THEN
        UPDATE public.devices AS d
        SET revoked_at = now(), revoked_reason = 'refresh_reused'
        WHERE d.tenant_id = matched.tenant_id AND d.id = matched.id;
        INSERT INTO public.audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata)
        VALUES (gen_random_uuid(), matched.tenant_id, matched.user_id, p_correlation_id,
                'devices.device.revoked', 'device:' || matched.id::text,
                jsonb_build_object('reason', 'refresh_reused'));
        RETURN QUERY SELECT matched.tenant_id, matched.id, matched.user_id, 'wipe'::text;
        RETURN;
    END IF;
    UPDATE public.device_tokens AS k SET used_at = now()
    WHERE k.tenant_id = token.tenant_id AND k.token_hash = token.token_hash;
    DELETE FROM public.device_tokens AS k
    WHERE k.tenant_id = matched.tenant_id AND k.device = matched.id AND k.expires_at < now();
    INSERT INTO public.device_tokens (tenant_id, token_hash, device, kind, created_at, expires_at)
    VALUES (matched.tenant_id, p_access_hash, matched.id, 'access', now(), p_access_expires_at),
           (matched.tenant_id, p_new_refresh_hash, matched.id, 'refresh', now(), p_refresh_expires_at);
    UPDATE public.devices AS d SET last_seen_at = now()
    WHERE d.tenant_id = matched.tenant_id AND d.id = matched.id;
    RETURN QUERY SELECT matched.tenant_id, matched.id, matched.user_id, 'issued'::text;
END
$$;

-- Workspace access for the member an unexpired access token's device
-- belongs to, in one workspace, and whether the device must wipe its
-- replica: it was revoked, or its member left. Mirrors the session and API
-- client routines: no rows for anything else, so it cannot enumerate other
-- workspaces.
CREATE FUNCTION platform_device_token_access(p_token_hash text, p_tenant_id uuid)
RETURNS TABLE (
    tenant_id uuid,
    display_name text,
    user_id uuid,
    device uuid,
    wipe boolean,
    role_name text,
    permission_id text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT t.id, t.display_name, d.user_id, d.id,
           (d.revoked_at IS NOT NULL OR m.user_id IS NULL),
           r.name, rp.permission_id
    FROM public.device_tokens AS k
    JOIN public.devices AS d ON d.tenant_id = k.tenant_id AND d.id = k.device
    JOIN public.tenants AS t ON t.id = d.tenant_id
    LEFT JOIN public.memberships AS m ON m.tenant_id = d.tenant_id AND m.user_id = d.user_id
    LEFT JOIN public.user_roles AS ur ON ur.tenant_id = m.tenant_id AND ur.user_id = m.user_id
    LEFT JOIN public.roles AS r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id
    LEFT JOIN public.role_permissions AS rp ON rp.tenant_id = r.tenant_id AND rp.role_id = r.id
    WHERE k.token_hash = p_token_hash
      AND k.tenant_id = p_tenant_id
      AND k.kind = 'access'
      AND k.expires_at > now()
    ORDER BY r.name, rp.permission_id
$$;

REVOKE ALL ON FUNCTION platform_refresh_device_token(text, text, text, timestamptz, timestamptz, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_refresh_device_token(text, text, text, timestamptz, timestamptz, uuid) TO platform_app;
REVOKE ALL ON FUNCTION platform_device_token_access(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_device_token_access(text, uuid) TO platform_app;
