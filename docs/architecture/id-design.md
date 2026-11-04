# Public ID design

## Salesforce pattern studied
Salesforce record IDs are compact and operationally useful because they carry an object key prefix and have a case-safe 18-character form for integrations. Salesforce Help documents conversion from 15-character IDs to 18-character IDs for systems that are not case-sensitive, and its key-prefix documentation maps leading characters to object types.

## Limiara goals
- Human-friendly enough for URLs, support tickets, imports, exports, and logs.
- Globally unambiguous in API context and unique within a tenant/workspace.
- Case-insensitive by construction; no mixed-case alphabet or Excel-sensitive uniqueness.
- Prefix identifies the plugin construct without querying every table.
- Checksum catches common copy/paste or transcription mistakes.
- Internal database keys can remain UUIDv7/composite keys while public IDs are stable integration handles.

## Format: 18 lowercase characters

```text
ppp ssssssssssss ccc
│   │            └─ 3-character checksum over prefix+payload
│   └────────────── 12-character tenant-local base32 sequence payload
└────────────────── 3-character construct prefix registered by plugin
```

Examples:
- `per000000000016abc` for a Contacts person
- `org00000000001zxyz` for a Contacts organisation

The alphabet is lowercase Crockford-style base32 without ambiguous letters: `0123456789abcdefghjkmnpqrstvwxyz`.

## Why this improves on the Salesforce idea
- Always case-insensitive; no separate 15-character and 18-character forms.
- Tenant-local uniqueness is explicit, so the same compact sequence space can be used per workspace while `(tenant_id, public_id)` remains unique.
- Prefixes are registered by plugin/construct, avoiding hardcoded brand/domain assumptions.
- Checksum is part of the canonical ID, not a compatibility suffix added later.
- UUIDv7 remains available internally for distributed generation and ordering; public IDs optimize operator ergonomics.

## Generation strategy
Use a tenant-local monotonically increasing sequence per construct or an allocator that leases sequence ranges to application nodes. The database enforces `(tenant_id, public_id)` uniqueness. Future distributed cells can allocate non-overlapping ranges per tenant/construct/cell.

## Security and privacy
Public IDs are not authorization tokens and must never replace permission checks. Sequential public IDs can leak approximate record counts if exposed broadly; for sensitive constructs use random payload mode or opaque UUID-only APIs. Contacts can use sequential public IDs because list/detail access is already permission and tenant scoped.
