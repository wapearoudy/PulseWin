import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { finalizeWindowsArtifacts } from './windows-artifacts.mjs';
import { publishLocalUpdates } from './update-feed.mjs';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const args = process.argv.slice(2);
const releaseBuild = process.platform === 'win32' && ['build', 'bundle'].includes(args[0]) &&
    !args.some(arg => ['--debug', '--help', '-h', '--version', '-V'].includes(arg));
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const environment = { ...process.env };
if (releaseBuild) {
  const key = path.join(homedir(), '.pulsewin-signing/updater.key');
  if (!environment.TAURI_SIGNING_PRIVATE_KEY && existsSync(key)) environment.TAURI_SIGNING_PRIVATE_KEY = key;
  if (!environment.TAURI_SIGNING_PRIVATE_KEY) throw new Error('Update signing key missing. Set TAURI_SIGNING_PRIVATE_KEY or restore ~/.pulsewin-signing/updater.key; do not replace the installed public key.');
  environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
  const channelFile=path.join(root,'release-channel.json');
  const channel=existsSync(channelFile)?JSON.parse(readFileSync(channelFile,'utf8')).source:path.join(root,'updates');
  writeFileSync(path.join(root, 'src-tauri/update-channel.txt'), channel);
}
const cli = spawnSync(process.execPath, [require.resolve('@tauri-apps/cli/tauri.js'), ...args], {
  stdio: 'inherit',
  env: environment,
});
if (cli.error) throw cli.error;
if (cli.status !== 0) process.exit(cli.status ?? 1);

// Preserve the normal CLI, then finalize the default Windows release outputs.
// Inherited Low Mandatory Level otherwise breaks NSIS before .onInit can run.
if (releaseBuild) {
  finalizeWindowsArtifacts();
  publishLocalUpdates(root);
}
