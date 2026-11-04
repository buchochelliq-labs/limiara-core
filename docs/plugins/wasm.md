# Wasm plugin contract
Wasm plugins use WIT (`plugins/hello-wasm/wit/hello.wit`) and default-deny host interfaces. Phase 1 grants only hello/health and structured host logging. No ambient filesystem, environment, database, secrets, or network access is available. Future host operations must be capability-mediated.
