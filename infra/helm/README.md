# Kubernetes packaging
Minimal Helm-oriented manifests are deferred to the next hardening pass. Deploy API, worker, web, and a migration Job with non-root containers, readiness/liveness probes, resource requests/limits, and NetworkPolicies allowing API/worker to PostgreSQL and NATS only.
