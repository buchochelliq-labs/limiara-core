# Tenant security tests

`tenant_isolation.sql` is the executable application-role RLS harness. It
refuses table-owner, superuser, `BYPASSRLS`, indirect `SET ROLE`, and non-
`platform_app` sessions before touching fixtures. The suite covers people,
organisations, custom-field definitions, cursor pagination, plugin
installations, audit, inbox, outbox, RBAC assignments, and job schedules with
deterministic two-workspace rows. It also proves the plugin execution context:
every plugin-schema table carries a restrictive `app.plugin_id` policy, and
another plugin's context (or none) cannot read Contacts rows, write them, write
Contacts inbox markers, or publish Contacts events.

Run it against an isolated database with:

```bash
bash scripts/test-tenant-rls.sh
```

On Windows PowerShell, run `./scripts/test-tenant-rls.ps1`. The migration gate
calls the same harness runner, so this coverage also remains part of the
existing isolated migration validation.

That gate proves rejection under privileged roles, reprovisions runtime roles,
then runs the suite directly as `platform_app`. It exercises known-ID reads,
zero-row mutations, cross-workspace inserts, enumeration and pagination probes,
and absent or malformed `app.tenant_id` values. The transaction always rolls
back its fixtures.

The outbox publisher uses a separate non-owner, `NOBYPASSRLS`
`platform_worker` role. It has no direct access to protected tables; its only
cross-workspace database capabilities are bounded claim, settlement, and
aggregate-backlog routines, plus startup-only interrupted-plugin recovery,
owned by the migrator role. The request-serving role cannot invoke them.
