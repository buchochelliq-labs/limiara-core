# Web dependency updates

The web workspace uses Node.js 22 and pnpm 9.12.0. Package versions are pinned in
`package.json` files and the complete dependency graph is committed in
`pnpm-lock.yaml`.

To update a dependency:

1. Use pnpm 9.12.0 and Node.js 22.
2. Run `pnpm update --save-exact <package> --filter web` from the repository root.
3. Review both the manifest and lockfile changes. Do not edit the lockfile by hand.
4. Run `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test`, and
   `pnpm build`.
5. Commit the package manifest and `pnpm-lock.yaml` together.

CI and the web container build use `pnpm install --frozen-lockfile`. They fail
when a manifest and the lockfile disagree, preventing an unreviewed dependency
resolution from entering a build.
