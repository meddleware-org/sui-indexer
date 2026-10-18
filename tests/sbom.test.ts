import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

// scripts/check-sbom.mjs is the release gate that keeps the attested npm SBOM (IMG-M8 / F18) complete.
const SCRIPT = join(import.meta.dirname, '..', 'scripts', 'check-sbom.mjs')

const lock = {
  packages: {
    '': { devDependencies: { typescript: '~6.0.3', vitest: '~5.0.2' } },
    'node_modules/@mysten/sui': { version: '2.35.0' },
    'node_modules/@mysten/sui/node_modules/nested': { version: '1.0.0' },
    'node_modules/typescript': { version: '6.0.3', peer: true },
    'node_modules/vitest': { version: '5.0.3', dev: true },
    'node_modules/fsevents': { version: '2.3.3', optional: true },
  },
}
const sbomOf = (...ids: string[]) => ({ packages: ids.map((id) => ({ name: id.slice(0, id.lastIndexOf('@')), versionInfo: id.slice(id.lastIndexOf('@') + 1) })) })

describe('check-sbom', () => {
  let dir: string
  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), 'sbom-'))
    writeFileSync(join(dir, 'package-lock.json'), JSON.stringify(lock))
  })
  afterAll(() => rmSync(dir, { recursive: true, force: true }))

  const run = (sbom: unknown) => {
    const file = join(dir, 'sbom.json')
    writeFileSync(file, JSON.stringify(sbom))
    return spawnSync(process.execPath, [SCRIPT, file, join(dir, 'package-lock.json')], { encoding: 'utf8' })
  }

  it('passes when every production package is listed (dev, optional and dev-peer entries are not expected)', () => {
    const r = run(sbomOf('@meddleware/sui-indexer@0.0.10', '@mysten/sui@2.35.0', 'nested@1.0.0'))
    expect(r.status).toBe(0)
    expect(r.stdout).toContain('lists all 2 production packages')
  })

  it('fails and names a bundled package the SBOM omits', () => {
    const r = run(sbomOf('@meddleware/sui-indexer@0.0.10', 'nested@1.0.0'))
    expect(r.status).toBe(1)
    expect(r.stderr).toContain('@mysten/sui@2.35.0')
  })

  it('fails on a version mismatch and on an SBOM without packages', () => {
    expect(run(sbomOf('@mysten/sui@2.34.0', 'nested@1.0.0')).status).toBe(1)
    expect(run({ packages: [] }).status).toBe(1)
  })
})
