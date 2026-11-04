# Performance tests

These SQL plan checks are intentionally lightweight and can run before a full load-test harness exists. They document expected access paths for tenant-scoped pagination and worker backlog polling. Later phases should add k6/Vegeta API tests and fixture generation for 10k, 100k, and 1M Contacts rows per workspace.
