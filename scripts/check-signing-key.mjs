#!/usr/bin/env node
// Does this private key match the public key the app already trusts?
//
// The updater's public half is pinned in src-tauri/tauri.conf.json and
// `updater.requireSignedVersion` is true, so a package signed by any other key
// is rejected by every installed client. Uploading the wrong file to the
// TAURI_SIGNING_PRIVATE_KEY secret does not fail the build - it produces
// installers nobody can accept - so the key is checked here first.
//
//   node scripts/check-signing-key.mjs ~/.pulsewin-signing/updater.key
//   TAURI_SIGNING_PRIVATE_KEY_PASSWORD=... node scripts/check-signing-key.mjs ...
//
// The key id cannot be read out of the private key: minisign stores the secret
// key encrypted, so its header carries the KDF salt where the public key
// carries the id. The only way to learn which key a file holds is to sign with
// it, which is what this does - into a temp file, thrown away afterwards.
//
// Exit 0 means the key is the one to upload; exit 1 means it is not.

import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

/// The 8-byte key id at the head of a minisign blob, as uppercase hex.
///
/// Read in file order, which is how a signature carries it. minisign's own
/// comment line prints the same bytes reversed (`minisign public key:
/// DCF3C4E240198D21` for the bytes `218D1940E2C4F3DC`).
function keyId(blob) {
  // algorithm ("Ed" public, "ED" private) then the 8-byte id.
  return [...blob.subarray(2, 10)].map(b => b.toString(16).padStart(2, '0')).join('').toUpperCase()
}

function lines(text) {
  return text.split(/\r?\n/).map(line => line.trim()).filter(Boolean)
}

/// A minisign file is a comment line then base64. Tauri writes the private key
/// with its own header, and the public key in tauri.conf.json is base64 of the
/// whole two-line file, so both forms are accepted.
function blob(text) {
  const body = lines(text.trim())
  if (body.length >= 2 && !body[0].startsWith('untrusted comment:')) {
    return Buffer.from(body[1], 'base64')
  }
  const decoded = lines(Buffer.from(text.trim(), 'base64').toString('utf8'))
  if (decoded.length >= 2 && decoded[0].startsWith('untrusted comment:')) {
    return Buffer.from(decoded[1], 'base64')
  }
  return Buffer.from(body[1] ?? text.trim(), 'base64')
}

// A file path, or the environment variable the build itself reads. The
// environment form is what the release workflow uses, so the key never has to
// be written to the runner's disk to be checked.
const target = process.argv[2]
const fromEnvironment = !target && Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY)
if (!target && !fromEnvironment) {
  console.error('usage: node scripts/check-signing-key.mjs <private key file>')
  console.error('   or: TAURI_SIGNING_PRIVATE_KEY=... node scripts/check-signing-key.mjs')
  process.exit(2)
}

const config = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8'))
const pinned = config.plugins?.updater?.pubkey ?? ''
if (blob(pinned).length < 10) {
  console.error('Could not read plugins.updater.pubkey from src-tauri/tauri.conf.json.')
  process.exit(2)
}
const wanted = keyId(blob(pinned))

let key
const described = fromEnvironment ? 'TAURI_SIGNING_PRIVATE_KEY' : target
if (fromEnvironment) {
  key = process.env.TAURI_SIGNING_PRIVATE_KEY
} else {
  try {
    key = readFileSync(target, 'utf8')
  } catch (error) {
    console.error(`Could not read ${target}: ${error.message}`)
    process.exit(2)
  }
}

const work = mkdtempSync(join(tmpdir(), 'pulsewin-key-check-'))
try {
  const probe = join(work, 'probe.txt')
  writeFileSync(probe, 'PulseWin signing key check\n')
  try {
    execFileSync(
      process.execPath,
      [join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign', probe],
      {
        cwd: root,
        env: {
          ...process.env,
          TAURI_SIGNING_PRIVATE_KEY: key,
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? ''
        },
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe']
      }
    )
  } catch (error) {
    const detail = String(error.stderr ?? error.message).trim()
    // A failure to launch the CLI is not a verdict on the key, so say which.
    if (error.code === 'ENOENT' || error.code === 'EINVAL') {
      console.error(`Could not run the Tauri CLI: ${detail || error.code}`)
      console.error('Run npm ci first; this reads node_modules/@tauri-apps/cli/tauri.js.')
      process.exit(2)
    }
    console.error(`${described} could not sign with this key.`)
    if (detail) console.error(detail.split('\n').slice(-3).join('\n'))
    console.error('If the key has a password, set TAURI_SIGNING_PRIVATE_KEY_PASSWORD first.')
    process.exit(2)
  }

  // `signer sign` also writes `<file>.sig`. Reading that file is exact: the
  // command's stdout carries the same base64 plus a trailing note, and picking
  // the base64 out of the note is how this got the wrong bytes once.
  let signature
  try {
    signature = readFileSync(`${probe}.sig`, 'utf8')
  } catch {
    console.error(`${described} produced no signature file.`)
    process.exit(2)
  }
  const signatureBlob = blob(signature)
  if (signatureBlob.length < 10) {
    console.error(`${described} produced no usable signature; it may not be a minisign key.`)
    process.exit(2)
  }
  const found = keyId(signatureBlob)
  if (found !== wanted) {
    console.error(`MISMATCH: this key signs as ${found}, the app trusts ${wanted}.`)
    console.error('Uploading it would sign updates no installed client accepts.')
    process.exit(1)
  }
  console.log(`MATCH: this key signs as ${wanted} - the key the app already trusts.`)
  console.log('Upload it as the repository secret TAURI_SIGNING_PRIVATE_KEY.')
} finally {
  rmSync(work, { recursive: true, force: true })
}
