-- Run as application role, not owner/superuser.
BEGIN;
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
SELECT set_config('app.plugin_id', 'platform.contacts', true);
-- Queries against tenant B rows must return zero rows and mutations must affect zero rows.
ROLLBACK;
