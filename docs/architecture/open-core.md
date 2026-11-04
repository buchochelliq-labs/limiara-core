# Open-core boundary
Limiara is licensed under the Apache License 2.0 (`LICENSE`).

Everything in this repository runs on its own: a self-hosted server, its plugins and the device apps need no Limiara-hosted service or control plane. Hosted offerings build on the same public APIs and plugin contracts.

Entitlements are centralised behind `EntitlementService` and are never authorization decisions. User permissions, plugin runtime capabilities and entitlements stay separate checks (ADR 0006).
