// Tests for the npm launcher, npm/brrrty/bin/brrrty. Run with `node --test npm/`.

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import test from 'node:test';

const here = path.dirname(fileURLToPath(import.meta.url));
const launcherPath = path.join(here, '..', 'brrrty', 'bin', 'brrrty');
const fake = path.join(here, 'fake-brrrty.cjs');
const require = createRequire(import.meta.url);
const launcher = require(launcherPath);

// Runs the launcher's run() on the fake binary in a separate Node process.
function launch(args) {
  const code = `require(${JSON.stringify(launcherPath)}).run(${JSON.stringify(fake)}, ${JSON.stringify(args)})`;
  const child = spawn(process.execPath, ['-e', code], { stdio: ['ignore', 'pipe', 'pipe'] });
  const out = { stdout: '', stderr: '' };
  child.stdout.on('data', (d) => (out.stdout += d));
  child.stderr.on('data', (d) => (out.stderr += d));
  const done = new Promise((resolve) =>
    child.on('exit', (code, signal) => resolve({ code, signal, ...out })),
  );
  const ready = new Promise((resolve) => {
    child.stdout.on('data', () => out.stdout.includes('ready\n') && resolve());
  });
  return { child, done, ready };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

test('every supported platform maps to a package', () => {
  assert.deepEqual(launcher.PACKAGES, {
    'darwin-arm64': '@brrrty/darwin-arm64',
    'darwin-x64': '@brrrty/darwin-x64',
    'linux-arm64': '@brrrty/linux-arm64-musl',
    'linux-x64': '@brrrty/linux-x64-musl',
  });
});

test('finds the binary in the platform package', () => {
  const resolve = (id) => {
    assert.equal(id, '@brrrty/linux-x64-musl/package.json');
    return '/n/node_modules/@brrrty/linux-x64-musl/package.json';
  };
  assert.equal(
    launcher.binaryPath('linux', 'x64', resolve),
    '/n/node_modules/@brrrty/linux-x64-musl/bin/brrrty',
  );
});

test('an unsupported platform is named in the error', () => {
  assert.throws(
    () => launcher.binaryPath('win32', 'x64', () => assert.fail('no lookup')),
    (err) =>
      err instanceof launcher.LauncherError &&
      err.message.includes('no prebuilt Brrrty binary for win32-x64') &&
      err.message.includes('cargo install brrrty'),
  );
});

test('a missing platform package is named in the error', () => {
  const resolve = () => {
    throw Object.assign(new Error('Cannot find module'), { code: 'MODULE_NOT_FOUND' });
  };
  assert.throws(
    () => launcher.binaryPath('darwin', 'arm64', resolve),
    (err) =>
      err instanceof launcher.LauncherError &&
      err.message.includes('for darwin-arm64') &&
      err.message.includes('@brrrty/darwin-arm64, which is not installed') &&
      err.message.includes('--omit=optional'),
  );
});

for (const status of [0, 1, 2, 101]) {
  test(`exit status ${status} and output bytes pass through`, async () => {
    const r = await launch(['exit', String(status)]).done;
    assert.equal(r.code, status);
    assert.equal(r.stdout, 'out ✓\n');
    assert.equal(r.stderr, 'err ⚠\n');
  });
}

test('arguments pass through unchanged', async () => {
  const args = ['build', '--input', 'a b', '', '--x=é', '-'];
  const r = await launch(['args', ...args]).done;
  assert.equal(r.code, 0);
  assert.deepEqual(JSON.parse(r.stdout), args);
});

test('the binary is told the launcher\'s process id', async () => {
  const l = launch(['env', 'BRRRTY_LAUNCHER_PID']);
  const r = await l.done;
  assert.equal(r.code, 0);
  assert.equal(r.stdout, String(l.child.pid));
});

test('the binary is told which Node runs the launcher', async () => {
  const r = await launch(['env', 'BRRRTY_NODE']).done;
  assert.equal(r.code, 0);
  assert.equal(r.stdout, process.execPath);
});

test('a BRRRTY_NODE already set is kept', async () => {
  const code = `require(${JSON.stringify(launcherPath)}).run(${JSON.stringify(fake)}, ["env", "BRRRTY_NODE"])`;
  const child = spawn(process.execPath, ['-e', code], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, BRRRTY_NODE: '/opt/node/bin/node' },
  });
  let stdout = '';
  child.stdout.on('data', (d) => (stdout += d));
  const status = await new Promise((resolve) => child.on('exit', resolve));
  assert.equal(status, 0);
  assert.equal(stdout, '/opt/node/bin/node');
});

test('every catchable terminating signal is forwarded', () => {
  for (const s of ['SIGHUP', 'SIGINT', 'SIGQUIT', 'SIGTERM']) assert.ok(launcher.FORWARDED.includes(s), s);
});

test('a binary that cannot run is reported', async () => {
  const code = `require(${JSON.stringify(launcherPath)}).run('/nonexistent/brrrty', [])`;
  const child = spawn(process.execPath, ['-e', code]);
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  const r = await new Promise((resolve) => child.on('exit', (code) => resolve({ code })));
  assert.equal(r.code, 1);
  assert.match(stderr, /^brrrty: could not run \/nonexistent\/brrrty: .*ENOENT/);
});

test('SIGINT reaches the binary once, even when it arrives twice at once', async () => {
  // npm forwards Ctrl-C to the launcher, which also gets it from the terminal.
  const l = launch(['serve']);
  await l.ready;
  l.child.kill('SIGINT');
  await sleep(30);
  l.child.kill('SIGINT');
  const r = await l.done;
  assert.equal(r.code, 0);
  assert.equal(r.signal, null);
  assert.equal(r.stdout, 'ready\nINT\n');
});

test('a second Ctrl-C later on reaches the binary too', async () => {
  const l = launch(['serve', '1500']);
  await l.ready;
  l.child.kill('SIGINT');
  await sleep(500);
  l.child.kill('SIGINT');
  const r = await l.done;
  assert.equal(r.code, 0);
  assert.equal(r.stdout, 'ready\nINT\nINT\n');
});

test('SIGQUIT reaches the binary, and its death passes through', async () => {
  const l = launch(['serve']);
  await l.ready;
  l.child.kill('SIGQUIT');
  const r = await l.done;
  assert.equal(r.stdout, 'ready\nQUIT\n');
  assert.equal(r.code, null);
  assert.equal(r.signal, 'SIGQUIT');
});

test('death by SIGTERM passes through as death by SIGTERM', async () => {
  const l = launch(['serve']);
  await l.ready;
  l.child.kill('SIGTERM');
  const r = await l.done;
  assert.equal(r.code, null);
  assert.equal(r.signal, 'SIGTERM');
});
