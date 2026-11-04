-- Custom-field metadata must be tenant isolated and API-addressable with __c names.
BEGIN;
-- Contacts tables also require the Contacts plugin execution context.
SELECT set_config('app.plugin_id', 'platform.contacts', true);
SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000001', true);
INSERT INTO p_contacts.custom_field_definitions (tenant_id, id, applies_to, api_name, label, field_type, required)
VALUES ('018f0000-0000-7000-8000-000000000001', '018f3000-0000-7000-8000-000000000001', 'person', 'service_tier__c', 'Service Tier', 'select', true);

SELECT set_config('app.tenant_id', '018f0000-0000-7000-8000-000000000002', true);
SELECT COUNT(*) AS custom_fields_visible_to_tenant_b_must_be_zero
FROM p_contacts.custom_field_definitions
WHERE api_name = 'service_tier__c';
ROLLBACK;
