# Plugin supply-chain controls

Core out-of-box plugins, such as `platform.contacts`, are privileged because operators tend to trust them by default, grant broad permissions, and deploy them together with the platform. A compromised build, release, dependency, or manifest for one of these plugins can become a tenant-wide data-exfiltration or privilege-escalation path. The same controls should also be available to first-party and third-party plugins, with stricter policy gates applied to higher-risk distributions.

## Threats considered

- A malicious package is substituted for a core plugin during build, release, container assembly, or installation.
- A legitimate plugin source repository is compromised and publishes a backdoored artifact.
- A dependency, build action, base image, or developer token injects code into a plugin artifact.
- A manifest is edited after review to request broader capabilities, outbound hosts, secrets, migrations, or event publishing.
- A third-party plugin impersonates a core plugin identifier or depends on a compromised plugin to gain trust by association.
- An attacker tampers with migrations or read-model definitions to bypass tenant isolation or leak data.

## Required controls for core out-of-box plugins

Core out-of-box plugins must declare complete `[supply_chain]` metadata in the plugin manifest. The manifest parser rejects a plugin marked `distribution = "core-oob"` unless it includes source repository, source revision, build provenance, artifact digest, artifact path, signature, and SBOM references. The artifact path is relative to the plugin package and cannot traverse outside it. For the currently linked native plugin it identifies the checked-in link descriptor; later packaging can point at the loadable component without changing the verification contract. The registry reads that exact file and persists the verified SHA-256 digest. This makes the trust decision explicit and keeps the Contacts plugin from being silently treated as an unverified local artifact.

The current Gate 1 registry enforces identity reservation, trusted repository
and provenance references, source revision shape, dependency validity, and the
loaded-artifact SHA-256 check. Signature transparency, automated SBOM policy,
and capability-diff approval remain later controls. The complete target is:

1. The plugin id is reserved for the platform namespace and cannot be claimed by marketplace or local plugins.
2. The artifact digest matches the artifact that is loaded or unpacked.
3. The signature verifies against a pinned platform release key or keyless transparency-log identity.
4. The provenance attestation links the artifact to the expected repository, workflow, source revision, and build inputs.
5. The SBOM is present, archived with the release, and scanned for known vulnerable or prohibited dependencies.
6. The manifest capability set is compared against a reviewed allowlist for the plugin and version.
7. Native plugins are built in CI from reviewed source; local native artifacts are not accepted in production.

## Baseline controls for every plugin

All plugins, regardless of distribution, should use the same policy shape:

- Default-deny permissions and host capabilities.
- Reserved plugin-id namespaces for platform-owned ids.
- Immutable artifact digests for installed versions.
- Signature verification for non-local installations.
- SBOM generation and vulnerability/license scanning before promotion.
- Capability diff review on upgrade, especially for database, secrets, outbound HTTP, migrations, event publishing, object storage, and UI navigation.
- Dependency graph validation with cycle rejection and explicit dependency reasons.
- Tenant-isolation tests for database migrations and read models.
- Runtime confinement appropriate to execution mode: Wasm sandboxing for Wasm plugins, mediated network and secrets access, and container/process isolation for future external-service plugins.
- Audit events for discovery, validation, staging, migration, registration, health checks, enablement, disablement, and verification failures.

## Upgrade and incident response expectations

Plugin upgrades should be staged, verified, and health-checked before enablement. If verification fails, the registry should leave the currently enabled version running, mark the candidate as failed, and emit an audit event that includes the failed control without logging secrets. Operators need a documented rollback path that re-pins the previous digest and records the reason for rollback.
