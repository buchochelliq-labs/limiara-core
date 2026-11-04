-- Performance smoke tests for the exact worker claim and metrics predicates.
EXPLAIN (COSTS, VERBOSE)
SELECT id, tenant_id, event_type, envelope
FROM outbox
WHERE published_at IS NULL
  AND quarantined_at IS NULL
  AND next_attempt_at <= now()
  AND (claim_expires_at IS NULL OR claim_expires_at <= now())
ORDER BY next_attempt_at, created_at, id
LIMIT 32;

EXPLAIN (COSTS, VERBOSE)
SELECT count(*),
       count(*) FILTER (
         WHERE next_attempt_at <= now()
           AND (claim_expires_at IS NULL OR claim_expires_at <= now())
       ),
       count(*) FILTER (WHERE claim_expires_at > now()),
       min(created_at)
FROM outbox
WHERE published_at IS NULL AND quarantined_at IS NULL;
