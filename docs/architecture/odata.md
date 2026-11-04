# OData endpoint strategy

OData is useful for enterprise reporting tools and low-code integrations that expect standardized metadata, entity sets, and query conventions. Limiara exposes OData as a governed read/API projection, not as raw table access.

Phase 1 adds workspace-scoped stubs:
- `GET /api/v1/workspaces/{workspace_id}/odata/$metadata`
- `GET /api/v1/workspaces/{workspace_id}/odata/contacts/people`

Rules:
- OData endpoints remain workspace-scoped and permission-checked.
- Disabled plugins must not expose their entity sets.
- `$metadata` must include standard fields and later tenant custom fields that the caller is allowed to see.
- `$filter`, `$select`, `$orderby`, `$top`, and `$skiptoken` should map to vetted query builders only.
- OData must not bypass RLS, plugin lifecycle state, audit policy, or rate limits.
