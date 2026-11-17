#!/usr/bin/env node
// Stands in for the Brrrty binary in launcher.test.mjs.
'use strict';

const [mode, ...rest] = process.argv.slice(2);

if (mode === 'exit') {
  process.stdout.write('out ✓\n');
  process.stderr.write('err ⚠\n');
  process.exitCode = Number(rest[0]);
} else if (mode === 'args') {
  process.stdout.write(JSON.stringify(rest));
} else if (mode === 'env') {
  process.stdout.write(String(process.env[rest[0]]));
} else if (mode === 'serve') {
  // Like `brrrty serve`: the first SIGINT asks for a clean stop that takes a
  // moment; SIGTERM keeps its default action. Every SIGINT is logged.
  const stopAfter = Number(rest[0] || 300);
  let stopping = false;
  process.on('SIGINT', () => {
    process.stdout.write('INT\n');
    if (!stopping) {
      stopping = true;
      setTimeout(() => process.exit(0), stopAfter);
    }
  });
  process.once('SIGQUIT', () => {
    process.stdout.write('QUIT\n', () => process.kill(process.pid, 'SIGQUIT'));
  });
  setInterval(() => {}, 1000);
  process.stdout.write('ready\n');
}
