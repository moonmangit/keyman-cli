#!/usr/bin/env node

const fs = require('node:fs');
const os = require('node:os');
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

const args = process.argv.slice(2);
let result = spawnSync(binary, args, { stdio: 'inherit' });
if (process.platform === 'win32' && result.error && ['UNKNOWN', 'EPERM', 'EACCES'].includes(result.error.code)) {
  try {
    result = runWithWsl(args);
  } catch (error) {
    console.error(`keyman: Windows blocked the native executable and WSL fallback failed: ${error.message}`);
    process.exit(1);
  }
}
if (result.error) {
  console.error(`keyman: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exitCode = result.status ?? 1;
}

function runWithWsl(args) {
  const linuxBinary = path.join(__dirname, 'native', 'x86_64-unknown-linux-gnu', 'keyman');
  if (!fs.existsSync(linuxBinary)) {
    throw new Error('the Linux x64 binary is missing from this package');
  }

  const linuxArgs = [...args];
  if (['load', 'init'].includes(linuxArgs[0]) && linuxArgs.length === 2 && !linuxArgs[1].startsWith('-')) {
    linuxArgs[1] = toWslPath(linuxArgs[1]);
  } else if (['inject', 'save', 'diff'].includes(linuxArgs[0]) && linuxArgs.length === 3 && !linuxArgs[2].startsWith('-')) {
    linuxArgs[2] = toWslPath(linuxArgs[2]);
  }

  const linuxHome = toWslPath(process.env.USERPROFILE || os.homedir());
  return spawnSync('wsl.exe', [
    '--exec', 'env', `HOME=${linuxHome}`, toWslPath(linuxBinary), ...linuxArgs,
  ], { stdio: 'inherit' });
}

function toWslPath(input) {
  const gitBashPath = /^\/([a-zA-Z])\//.exec(input);
  const windowsPath = gitBashPath
    ? `${gitBashPath[1].toUpperCase()}:${input.slice(2)}`
    : path.win32.resolve(input);
  const converted = spawnSync('wsl.exe', ['--exec', 'wslpath', '-u', windowsPath], {
    encoding: 'utf8',
  });
  if (converted.error || converted.status !== 0) {
    throw new Error(converted.error?.message || converted.stderr?.trim() || 'wslpath could not convert the path');
  }
  return converted.stdout.replace(/\r?\n$/, '');
}
