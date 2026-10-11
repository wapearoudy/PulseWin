import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'

export function releaseHeld(root) {
  const file=path.join(root,'docs/release-progress.json')
  return existsSync(file) && JSON.parse(readFileSync(file,'utf8')).releaseHold===true
}
export function assertReleaseReady(root) {
  if(releaseHeld(root)) throw new Error('Publication is on hold until feature implementation and validation are complete. See docs/release-progress.json.')
}
