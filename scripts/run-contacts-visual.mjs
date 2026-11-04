import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {createServer} from '../apps/web/node_modules/vite/dist/node/index.js';

const root = fileURLToPath(new URL('..', import.meta.url));
const server = await createServer({
  root: fileURLToPath(new URL('../apps/web', import.meta.url)),
  logLevel: 'error',
  server: {host: '127.0.0.1', port: 4176, strictPort: true},
});

try {
  await server.listen();
  const cli = fileURLToPath(new URL('../node_modules/@playwright/test/cli.js', import.meta.url));
  const child = spawn(process.execPath, [cli, 'test', '--config=playwright.contacts.config.ts', ...process.argv.slice(2)], {cwd: root, stdio: 'inherit'});
  const exitCode = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => resolve(code ?? 1));
  });
  process.exitCode = exitCode;
} finally {
  await server.close();
}
