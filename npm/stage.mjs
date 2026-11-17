#!/usr/bin/env node
// Lays out the npm packages for one release, ready for `npm pack` or
// `npm publish`:
//
//   node npm/stage.mjs --binaries DIR --out DIR [--tag vX.Y.Z]
//   node npm/stage.mjs --print-version [--tag vX.Y.Z]
//
// The second form only checks the version and prints it. DIR holds one `<rust target>/brrrty` per platform. Every package gets the
// version of the crates, and the run fails before writing anything when the
// crates disagree, when --tag is not `v<that version>`, or when a binary this
// machine can run reports another version.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const npmDir = path.dirname(fileURLToPath(import.meta.url));
const repo = path.dirname(npmDir);
const { PACKAGES } = createRequire(import.meta.url)('./brrrty/bin/brrrty');

// Launcher platform key to the Rust target whose binary it ships.
export const TARGETS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-musl',
  'linux-x64': 'x86_64-unknown-linux-musl',
};

function fail(message) {
  console.error(`stage: ${message}`);
  process.exit(1);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

// The `version` of every crate under crates/, which must all agree.
export function crateVersion(root = repo) {
  const crates = path.join(root, 'crates');
  const versions = new Map();
  for (const name of fs.readdirSync(crates).sort()) {
    const manifest = path.join(crates, name, 'Cargo.toml');
    if (!fs.existsSync(manifest)) continue;
    const pkg = fs.readFileSync(manifest, 'utf8').split(/^\[/m).find((s) => s.startsWith('package]'));
    const m = pkg && pkg.match(/^version\s*=\s*"([^"]+)"/m);
    if (!m) throw new Error(`no version in ${manifest}`);
    versions.set(name, m[1]);
  }
  const distinct = new Set(versions.values());
  if (distinct.size !== 1) {
    const list = [...versions].map(([n, v]) => `${n} ${v}`).join(', ');
    throw new Error(`the crates disagree on their version: ${list}`);
  }
  return [...distinct][0];
}

export function checkTag(tag, version) {
  if (tag !== `v${version}`) {
    throw new Error(`the tag ${tag} does not match the crates' version ${version} (expected v${version})`);
  }
}

function main() {
  const { values } = parseArgs({
    options: {
      binaries: { type: 'string' },
      out: { type: 'string' },
      tag: { type: 'string' },
      'print-version': { type: 'boolean', default: false },
    },
  });
  if (values['print-version']) {
    try {
      const version = crateVersion();
      if (values.tag !== undefined) checkTag(values.tag, version);
      console.log(version);
    } catch (err) {
      fail(err.message);
    }
    return;
  }
  if (!values.binaries || !values.out) fail('usage: node npm/stage.mjs --binaries DIR --out DIR [--tag vX.Y.Z]');

  if (Object.keys(TARGETS).sort().join() !== Object.keys(PACKAGES).sort().join()) {
    fail('TARGETS in stage.mjs and PACKAGES in the launcher list different platforms');
  }

  let version;
  try {
    version = crateVersion();
    if (values.tag !== undefined) checkTag(values.tag, version);
  } catch (err) {
    fail(err.message);
  }

  const mainPkg = readJson(path.join(npmDir, 'brrrty', 'package.json'));
  const listed = Object.keys(mainPkg.optionalDependencies).sort().join();
  if (listed !== Object.values(PACKAGES).sort().join()) {
    fail('optionalDependencies in npm/brrrty/package.json differ from the launcher\'s PACKAGES');
  }

  // Check every binary before writing anything.
  const binaries = {};
  for (const [key, target] of Object.entries(TARGETS)) {
    const bin = path.join(values.binaries, target, 'brrrty');
    if (!fs.existsSync(bin)) fail(`missing the ${key} binary: ${bin}`);
    if (key === `${process.platform}-${process.arch}`) {
      const said = execFileSync(bin, ['--version'], { encoding: 'utf8' }).trim();
      if (said !== `brrrty ${version}`) fail(`${bin} --version says "${said}", expected "brrrty ${version}"`);
    }
    binaries[key] = bin;
  }

  const out = path.resolve(values.out);
  fs.rmSync(out, { recursive: true, force: true });
  const license = path.join(repo, 'LICENSE');
  const template = readJson(path.join(npmDir, 'platform', 'package.json'));
  const readme = fs.readFileSync(path.join(npmDir, 'platform', 'README.md'), 'utf8');

  for (const [key, pkgName] of Object.entries(PACKAGES)) {
    const [os, cpu] = key.split('-');
    const platform = pkgName.slice('@brrrty/'.length);
    const dir = path.join(out, platform);
    fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
    const manifest = structuredClone(template);
    manifest.name = pkgName;
    manifest.version = version;
    manifest.description = template.description.replaceAll('PLATFORM', platform);
    manifest.os = [os];
    manifest.cpu = [cpu];
    writeJson(path.join(dir, 'package.json'), manifest);
    fs.writeFileSync(path.join(dir, 'README.md'), readme.replaceAll('PLATFORM', platform));
    fs.copyFileSync(license, path.join(dir, 'LICENSE'));
    const bin = path.join(dir, 'bin', 'brrrty');
    fs.copyFileSync(binaries[key], bin);
    fs.chmodSync(bin, 0o755);
  }

  const dir = path.join(out, 'brrrty');
  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  mainPkg.version = version;
  for (const dep of Object.keys(mainPkg.optionalDependencies)) mainPkg.optionalDependencies[dep] = version;
  writeJson(path.join(dir, 'package.json'), mainPkg);
  fs.copyFileSync(path.join(npmDir, 'brrrty', 'README.md'), path.join(dir, 'README.md'));
  fs.copyFileSync(license, path.join(dir, 'LICENSE'));
  fs.copyFileSync(path.join(npmDir, 'brrrty', 'bin', 'brrrty'), path.join(dir, 'bin', 'brrrty'));
  fs.chmodSync(path.join(dir, 'bin', 'brrrty'), 0o755);

  console.log(`stage: brrrty ${version} and ${Object.keys(PACKAGES).length} platform packages in ${out}`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
