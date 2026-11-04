-- Performance smoke test for Contacts list/search access path.
-- Expected: planner can use people_tenant_name_idx for tenant-scoped deterministic pagination.
EXPLAIN (COSTS, VERBOSE)
SELECT id, full_name, email, version
FROM p_contacts.people
WHERE tenant_id = '018f0000-0000-7000-8000-000000000001'
  AND archived_at IS NULL
  AND lower(full_name) >= lower('a')
ORDER BY lower(full_name), id
LIMIT 50;
