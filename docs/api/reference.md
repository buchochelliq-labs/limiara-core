# API reference proof

The repository-owned [OpenAPI description](./openapi.json) is the canonical machine-readable source. This bounded page proves that a specification can live beside the prose documentation without exposing workspace, plugin, or readiness internals.

## Check liveness

`GET` `/health/live`

Operation ID: `getLiveness`

Confirms that the API process is live. It does not reveal dependency, tenant, or plugin state.

### Response `200`

```json
{
  "status": "ok"
}
```

Interactive request execution is intentionally absent from the public proof. It should only be enabled after the API has a documented public origin, authentication policy, CORS policy, and rate limits.
