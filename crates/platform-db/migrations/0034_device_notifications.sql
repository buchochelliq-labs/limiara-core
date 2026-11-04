-- The worker's push notification dispatcher (ADR 0030 §3, ADR 0024
-- "Continual sync"). A device with a push token is told "workspace changed"
-- when its workspace's change log or metadata version moves past what it
-- was last told about, at most once per quiet window, so a burst of changes
-- costs one notification.

-- What each device was last told about. Registering a push token sets them
-- to the workspace's current position, since the app has just synced.
ALTER TABLE devices
    ADD COLUMN notified_position bigint NOT NULL DEFAULT 0 CHECK (notified_position >= 0),
    ADD COLUMN notified_metadata_version bigint NOT NULL DEFAULT 0 CHECK (notified_metadata_version >= 0),
    -- When the dispatcher last claimed the device, whether or not the
    -- provider delivered.
    ADD COLUMN notified_at timestamptz;

CREATE INDEX devices_push_due_idx ON devices (notified_at NULLS FIRST, tenant_id, id)
    WHERE push_token IS NOT NULL AND revoked_at IS NULL;

-- Claims up to p_limit devices to notify, across workspaces: the only
-- cross-tenant step. A device is due when it has a push token, is not
-- revoked, its member still belongs to the workspace, the workspace moved
-- past what the device was told, and the device was not claimed within the
-- last p_quiet_seconds. Claiming stamps notified_at, so concurrent workers
-- skip each other's devices and a provider outage is retried once per
-- quiet window rather than on every pass. Each row carries the position and
-- metadata version to record once the provider accepts the notification.
CREATE FUNCTION platform_claim_device_notifications(p_limit integer, p_quiet_seconds integer)
RETURNS TABLE (
    tenant_id uuid,
    device uuid,
    push_provider text,
    push_token text,
    change_position bigint,
    metadata_version bigint
)
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    WITH due AS (
        SELECT d.tenant_id, d.id, c.last_position, c.metadata_version
        FROM public.devices AS d
        JOIN public.change_counters AS c ON c.tenant_id = d.tenant_id
        WHERE d.push_token IS NOT NULL
          AND d.revoked_at IS NULL
          AND (c.last_position > d.notified_position
               OR c.metadata_version > d.notified_metadata_version)
          AND (d.notified_at IS NULL
               OR d.notified_at <= now() - make_interval(secs => greatest(p_quiet_seconds, 1)))
          AND EXISTS (
              SELECT 1 FROM public.memberships AS m
              WHERE m.tenant_id = d.tenant_id AND m.user_id = d.user_id
          )
        ORDER BY d.notified_at NULLS FIRST, d.tenant_id, d.id
        LIMIT least(greatest(p_limit, 1), 500)
        FOR UPDATE OF d SKIP LOCKED
    )
    UPDATE public.devices AS d
    SET notified_at = now()
    FROM due
    WHERE d.tenant_id = due.tenant_id AND d.id = due.id
    RETURNING d.tenant_id, d.id, d.push_provider, d.push_token, due.last_position, due.metadata_version
$$;

REVOKE ALL ON FUNCTION platform_claim_device_notifications(integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION platform_claim_device_notifications(integer, integer) TO platform_worker;
