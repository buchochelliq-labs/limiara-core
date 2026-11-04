---
applyTo: "**/*"
---

# Limiara platform instructions

Follow `.github/copilot-instructions.md` for repository-wide guidance. The most important constraints are: keep the kernel domain-neutral, implement business features as plugins, preserve tenant isolation with forced RLS, keep permissions/capabilities/entitlements separate, and do not execute arbitrary plugin JavaScript in the authenticated frontend origin.
