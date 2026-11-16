// What the generators in this folder share: the Prism and plugin versions
// Brrrty reproduces, the fence languages its grammars are made for, and
// loading Prism the way the plugin does. See README.md for upgrading.

'use strict';

const path = require('path');

const PRISM = '1.30.0';
const PLUGIN = '5.0.2';

// Every fence language on the test sites, as written in the info strings.
const FENCES = [
  'shell', 'php', 'javascript', 'sql', 'bash', 'conf', 'yaml', 'typescript',
  'json', 'ruby', 'python', 'markdown', 'jql', 'ini', 'html', 'css',
  'console', 'vim', 'liquid', 'c', 'xml', 'nginx', 'config', 'text', 'perl',
  'less', 'jsx', 'go', 'zsh', 'toml', 'terraform', 'rust', 'r', 'lua', 'js',
  'fstab', 'sh',
];

// Prism (`require("prismjs")`: core, markup, css, clike, javascript), the
// plugin's PrismLoader and PrismNormalizeAlias, and Prism's components.json,
// from <node_modules>. Throws unless the versions are the pinned ones.
function load(nm) {
  const plugin = path.join(nm, '@11ty/eleventy-plugin-syntaxhighlight');
  const prismDir = path.dirname(require.resolve('prismjs/package.json', { paths: [plugin] }));
  const versions = {
    prism: require(path.join(prismDir, 'package.json')).version,
    plugin: require(path.join(plugin, 'package.json')).version,
  };
  if (versions.prism !== PRISM || versions.plugin !== PLUGIN) {
    throw new Error(`need prismjs ${PRISM} and the plugin ${PLUGIN}, found ${JSON.stringify(versions)}`);
  }
  const Prism = require(require.resolve('prismjs', { paths: [plugin] }));
  const PrismLoader = require(path.join(plugin, 'src/PrismLoader.js'));
  const normalize = require(path.join(plugin, 'src/PrismNormalizeAlias.js'));
  const components = require(path.join(prismDir, 'components.json'));
  return { Prism, PrismLoader, normalize, components, versions, plugin };
}

module.exports = { PRISM, PLUGIN, FENCES, load };
