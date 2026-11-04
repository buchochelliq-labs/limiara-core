# External service plugins
`proto/plugin/v1/plugin.proto` defines v1 handshake, health, configuration validation, event delivery acknowledgement, error fields, and graceful shutdown. Production use must add workload identity, mTLS, capability grants, NetworkPolicy isolation, resource quotas, and mediated secrets. Full supervision and marketplace installation are deferred.
