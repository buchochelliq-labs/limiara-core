# 0003-postgresql-rls-tenancy: postgresql-rls-tenancy

## Context
Phase 0/1 needs a secure vertical slice without premature distributed-system complexity.

## Decision
Adopt the requested architecture boundary for this topic with versioned contracts, explicit tenant context, auditability, and local-first community operation.

## Consequences
The platform remains extractable later while the first release is operable as a modular monolith. Some enterprise/cloud concerns are documented extension points rather than runtime features.

## Alternatives considered
A microservice-per-plugin design, brand-coupled package names, unrestricted plugin access, and proprietary-control-plane dependency were rejected.

## Future revisit triggers
Revisit when scale, regulatory isolation, marketplace signing, or independent plugin release cadence creates measurable pressure.
