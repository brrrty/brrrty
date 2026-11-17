#!/usr/bin/env node
// Publishes the `npm pack` tarballs in a folder, the same ones the package
// tests ran against:
//
//   node npm/publish.mjs PACKS_DIR [--dry-run] [--provenance]
//
// The platform packages go first and the main `brrrty` package last, so the
// main package never points at a binary that isn't on npm yet. A package
// already on npm at this version is skipped, so a run that failed partway can
// simply be run again.

import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    'dry-run': { type: 'boolean', default: false },
    provenance: { type: 'boolean', default: false },
  },
});
if (positionals.length !== 1) {
  console.error('usage: node npm/publish.mjs PACKS_DIR [--dry-run] [--provenance]');
  process.exit(1);
}
const packs = path.resolve(positionals[0]);

function fail(message) {
  console.error(`publish: ${message}`);
  process.exit(1);
}

// Whether name@version is on the registry. Anything but a clear yes or a
// clear "not found" stops the run.
function published(name, version) {
  const r = spawnSync('npm', ['view', `${name}@${version}`, 'version', '--json'], { encoding: 'utf8' });
  if (r.status === 0) {
    // An unknown version of a known package prints nothing.
    return r.stdout.trim() !== '';
  }
  if (/\bE404\b/.test(r.stderr)) return false;
  fail(`npm view ${name}@${version} failed:\n${r.stderr}`);
}

const tarballs = fs
  .readdirSync(packs)
  .filter((f) => f.endsWith('.tgz'))
  .map((f) => {
    const file = path.join(packs, f);
    const manifest = JSON.parse(execFileSync('tar', ['-xzOf', file, 'package/package.json'], { encoding: 'utf8' }));
    return { file, name: manifest.name, version: manifest.version };
  });

// Every package must carry the license and the notices of the code it ports.
for (const t of tarballs) {
  const files = execFileSync('tar', ['-tzf', t.file], { encoding: 'utf8' }).split('\n');
  for (const f of ['package/LICENSE', 'package/NOTICE']) {
    if (!files.includes(f)) fail(`${path.basename(t.file)} has no ${f.slice('package/'.length)}`);
  }
}

const main = tarballs.filter((t) => t.name === 'brrrty');
const platforms = tarballs.filter((t) => t.name.startsWith('@brrrty/')).sort((a, b) => (a.name < b.name ? -1 : 1));
if (main.length !== 1) fail(`expected one brrrty tarball in ${packs}, found ${main.length}`);
if (main.length + platforms.length !== tarballs.length) {
  fail(`unexpected tarballs in ${packs}: ${tarballs.map((t) => t.name).join(', ')}`);
}

// The main package must depend on exactly these platform packages, and every
// version must be the main package's.
const mainManifest = JSON.parse(execFileSync('tar', ['-xzOf', main[0].file, 'package/package.json'], { encoding: 'utf8' }));
const version = mainManifest.version;
const deps = mainManifest.optionalDependencies || {};
const depNames = Object.keys(deps).sort().join(', ');
const packNames = platforms.map((t) => t.name).join(', ');
if (depNames !== packNames) fail(`brrrty depends on ${depNames}, but the platform tarballs are ${packNames}`);
for (const [name, spec] of Object.entries(deps)) {
  if (spec !== version) fail(`brrrty ${version} depends on ${name}@${spec}`);
}
for (const t of platforms) {
  if (t.version !== version) fail(`${t.name} is ${t.version}, but brrrty is ${version}`);
}

for (const t of [...platforms, main[0]]) {
  if (published(t.name, t.version)) {
    console.log(`publish: ${t.name}@${t.version} is already on npm, skipping`);
    continue;
  }
  const args = ['publish', t.file, '--access', 'public'];
  if (values['dry-run']) args.push('--dry-run');
  if (values.provenance) args.push('--provenance');
  console.log(`publish: npm ${args.join(' ')}`);
  const r = spawnSync('npm', args, { stdio: 'inherit' });
  if (r.status !== 0) {
    fail(`${t.name}@${t.version} failed; nothing after it was published. Run this again to resume.`);
  }
}
