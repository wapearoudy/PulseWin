import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function finalizeWindowsArtifacts() {
  if (process.platform !== 'win32') return;
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const { version } = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  if (!/^[\dA-Za-z.+-]+$/.test(version)) throw new Error('Invalid artifact version');
  const release = join(root, 'src-tauri/target/release');
  const paths = [
    join(release, 'pulsewin.exe'),
    join(release, `bundle/nsis/PulseWin_${version}_x64-setup.exe`),
    join(release, `bundle/msi/PulseWin_${version}_x64_en-US.msi`),
  ].filter(existsSync);
  if (!paths.length) throw new Error('No default x64 release artifacts found.');

  for (const path of paths) {
    if (!lstatSync(path).isFile()) throw new Error(`Refusing a linked artifact: ${path}`);
    // Only generated files, never the workspace or system TEMP permissions.
    // Low-labelled executables cannot write to normal TEMP or user app data.
    const result = spawnSync(join(process.env.SystemRoot, 'System32/icacls.exe'),
      [path, '/setintegritylevel', 'Medium'], { stdio: 'inherit' });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`Cannot finalize artifact integrity: ${path}`);
  }
  console.log(`Finalized ${paths.length} Windows release artifacts at normal Medium integrity.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  finalizeWindowsArtifacts();
}
