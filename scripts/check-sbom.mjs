#!/usr/bin/env node
// Fails unless an SPDX SBOM lists every production npm package in package-lock.json at its locked version.
// The image ships one esbuild bundle, so the image SBOM sees only OS packages (IMG-M8); the release
// workflow therefore also attests the SBOM `npm sbom --omit dev` writes, and this check keeps that
// document honest: a missing bundled package (or an empty SBOM) stops the release.
//
//   node scripts/check-sbom.mjs <sbom.spdx.json> [package-lock.json]
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const [sbomPath, lockPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'package-lock.json')] = process.argv.slice(2)
if (!sbomPath) {
  console.error('usage: check-sbom.mjs <sbom.spdx.json> [package-lock.json]')
  process.exit(2)
}

const sbom = JSON.parse(readFileSync(sbomPath, 'utf8'))
const lock = JSON.parse(readFileSync(lockPath, 'utf8'))
if (!Array.isArray(sbom.packages) || sbom.packages.length === 0) {
  console.error(`${sbomPath}: no packages`)
  process.exit(1)
}

const listed = new Set(sbom.packages.map((p) => `${p.name}@${p.versionInfo}`))
// A root devDependency that is also an (uninstalled) peer of a production package is locked without
// `dev: true`; `npm sbom --omit dev` rightly leaves it out, so it is not expected here.
const rootDev = new Set(Object.keys(lock.packages['']?.devDependencies ?? {}))
const expected = Object.entries(lock.packages)
  .filter(([path, p]) => path.startsWith('node_modules/') && !p.dev && !p.optional)
  .filter(([path, p]) => !(p.peer && rootDev.has(path.slice('node_modules/'.length))))
  .map(([path, p]) => `${path.slice(path.lastIndexOf('node_modules/') + 'node_modules/'.length)}@${p.version}`)
const missing = expected.filter((id) => !listed.has(id))
if (expected.length === 0 || missing.length > 0) {
  console.error(`${sbomPath}: ${expected.length === 0 ? 'the lockfile lists no production packages' : `missing ${missing.length} of ${expected.length} production packages: ${missing.join(', ')}`}`)
  process.exit(1)
}
console.log(`${sbomPath}: lists all ${expected.length} production packages`)
