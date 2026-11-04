# Competitor parity check: customisation, privacy, and integrations

For product interaction patterns, responsive behaviour, and the first Contacts
screen/state inventory, see the [Contacts UX benchmark and interaction brief](../product/contacts-ux-benchmark.md).

## Pipedrive
Public Pipedrive API docs describe custom fields for deals, organisations, people, products, and projects, with multiple field types. Pipedrive support docs also describe grouping custom fields and making fields visible in add views/import mapping.

Parity implications:
- Contacts people and organisations need custom fields from the first vertical slice.
- Field definitions need type, label, options, visibility/display metadata, and grouping later.
- Imports and list views must understand custom fields, not treat them as opaque JSON.

## Joblogic
Joblogic public docs describe Attributes/Custom Fields across assets, customers, sites, jobs, quotes, PPM contracts, projects, suppliers, and parts.

Parity implications:
- Customisation cannot be CRM-only; the plugin contract must support plugin-scoped custom fields for later jobs, assets, visits, stock, quotes, and invoices.
- Field metadata must be tenant-specific because operational workflows vary heavily by customer.


## Salesforce
Salesforce is the strongest architectural oracle for metadata-driven CRM platforms. Public Salesforce docs describe standard objects/fields, custom objects, custom fields, metadata APIs, custom metadata types, relationship fields, external-object patterns, and event-oriented extension concepts.

Parity implications:
- Standard Contacts fields should stay typed and indexed while tenant custom fields remain metadata-addressable.
- Admin customisation needs API names, deployable metadata, audit, permissioning, validation rules, and environment promotion.
- Cross-object relationships must be explicit metadata and must not bypass plugin ownership boundaries.
- Integration and reporting endpoints should expose governed metadata/projections rather than private tables.

## OData/integration parity
OData 4.01 is standardized by OASIS and widely used for enterprise data access. Supporting workspace-scoped OData projections helps reporting and integration parity without exposing raw database tables.

## Gaps intentionally deferred
- Formula fields
- Dependency rules between fields
- Field-level permissions/visibility rules
- Import mapping UI
- Generated indexed projections for high-volume custom filters
- Full OData query language support
