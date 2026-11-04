-- Performance smoke test for tenant-scoped custom field containment queries.
EXPLAIN (COSTS, VERBOSE)
SELECT id, full_name, custom_fields
FROM p_contacts.people
WHERE tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND custom_fields @> '{"service_tier__c":"gold"}'::jsonb
ORDER BY lower(full_name), id
LIMIT 50;
