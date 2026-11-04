# OData contract checks

- `GET /api/v1/workspaces/{workspace_id}/odata/$metadata` returns XML metadata for enabled plugin projections.
- `GET /api/v1/workspaces/{workspace_id}/odata/contacts/people` returns an OData JSON object with `@odata.context` and `value`.
- Disabled Contacts plugin state must return the same not-found/problem response as native Contacts routes.
- Future `$filter`, `$select`, `$orderby`, `$top`, and `$skiptoken` support must be allowlisted and covered by tenant isolation tests.
