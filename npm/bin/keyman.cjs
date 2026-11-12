#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const targets = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

const platform = `${process.platform}-${process.arch}`;
const target = targets[platform];
if (!target) {
  console.error(`keyman: unsupported platform ${platform}`);
  process.exit(1);
}

const binary = path.join(__dirname, 'native', target, `keyman${process.platform === 'win32' ? '.exe' : ''}`);
if (!fs.existsSync(binary)) {
  console.error(`keyman: binary for ${platform} is missing from this package`);
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`keyman: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exitCode = result.status ?? 1;
}
