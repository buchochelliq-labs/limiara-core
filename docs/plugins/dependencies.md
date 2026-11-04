# Plugin dependency reasons

A plugin dependency is not just load order. It must declare why the dependency exists so the registry can produce actionable diagnostics and future installers can explain impact to admins.

Dependency reasons:
- `composition`: the plugin assembles another plugin's API/UI as part of a larger application.
- `extension`: the plugin contributes fields, actions, timeline cards, or views to another plugin.
- `event-contract`: the plugin consumes a published event schema and needs it to remain compatible.
- `read-model`: the plugin depends on a governed read model exported by another plugin.
- `capability-provider`: the dependency provides a host capability such as email sending, storage, search, or payments.
- `migration-order`: the dependency must migrate first because foreign keys or read-model refreshes require it.

Dependencies must include a semantic-version requirement and one or more reasons. Cycles are rejected even when the reason is UI composition, because lifecycle operations must remain deterministic.
