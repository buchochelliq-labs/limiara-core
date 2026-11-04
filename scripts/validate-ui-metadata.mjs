import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const cargo = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
const stages = [
  {
    label: 'Validate representative plugin metadata',
    command: cargo,
    args: ['run', '--locked', '--quiet', '-p', 'ui-metadata-validator', '--', 'fixtures/plugins/ui-conformance/plugin.toml'],
  },
  {
    label: 'Run Rust metadata contract and security suites',
    command: cargo,
    args: ['test', '--locked', '-p', 'plugin-contract', '-p', 'ui-metadata', '-p', 'ui-metadata-delivery', '-p', 'ui-metadata-validator'],
  },
  {
    label: 'Run browser contract and host conformance suites',
    command: process.execPath,
    args: [
      'apps/web/node_modules/vitest/vitest.mjs', 'run', '--root', 'apps/web', '--passWithNoTests',
      'contracts.test.ts', 'MetadataPageRenderer.test.tsx', 'interactions.test.tsx', 'metadataConformance.test.tsx',
    ],
  },
];

for (const stage of stages) {
  process.stdout.write(`\n==> ${stage.label}\n`);
  const result = spawnSync(stage.command, stage.args, {cwd: root, env: process.env, stdio: 'inherit'});
  if (result.error) {
    process.stderr.write(`Could not start ${stage.command}: ${result.error.message}\n`);
    process.exit(1);
  }
  if (result.status !== 0) process.exit(result.status ?? 1);
}

process.stdout.write('\nUI metadata conformance gate passed.\n');
