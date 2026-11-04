# Customisation model for out-of-the-box constructs

## Salesforce pattern studied
Salesforce distinguishes standard fields/objects from administrator-created metadata in its APIs by appending `__c` to custom object and custom field API names. Relationship traversals commonly use related API-name conventions such as `__r`. The important architectural lesson is not the exact suffix; it is that customization is metadata-driven, API-addressable, and separated from standard platform fields.

## Limiara decision
Limiara will support tenant-defined custom fields on first-party plugin constructs, starting with Contacts people and organisations. We intentionally adopt a familiar `__c` suffix for tenant-defined field API names because it is explicit, deterministic, and prevents accidental collision with first-party fields.

Examples:
- Standard field: `full_name`
- Tenant custom field: `service_tier__c`
- Tenant custom field: `preferred_engineer__c`

## Backend implementation
Each plugin owns its private table and a metadata table for its custom fields. Contacts stores definitions in `p_contacts.custom_field_definitions` and values in each record's `custom_fields` JSONB column. This avoids a global EAV model while still allowing tenants to extend out-of-the-box constructs.

A custom field definition includes:
- `tenant_id`
- `applies_to` (`person` or `organisation`)
- `api_name`, which must end in `__c`
- label, description, type, required flag, uniqueness flag, index hint, options, and JSON Schema
- audit-friendly timestamps and actor references

## Query/index approach
The first implementation keeps values in JSONB and adds GIN indexes for broad containment queries. Frequently filtered or sorted custom fields should be promoted to generated columns or plugin-owned projection tables after measurement. Migration review must document expected filter/sort patterns before adding a custom-field index.

## API and UI behavior
The API must validate incoming `custom_fields` against tenant definitions before write. The UI should render custom fields from metadata, keep standard and custom fields visually grouped, and clearly identify admin-created fields without showing implementation jargon such as JSONB.

## Non-goals
Do not implement a universal object engine, cross-plugin raw table access, or arbitrary schema changes by tenants. Customisation remains plugin-scoped and governed by plugin contracts, permissions, audit, and tenant isolation.
