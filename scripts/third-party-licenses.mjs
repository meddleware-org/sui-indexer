#!/usr/bin/env node
// Writes dist/THIRD_PARTY_LICENSES: the licence and NOTICE texts of every production dependency in
// package-lock.json. The image ships one esbuild bundle, so the packages' own files do not travel with
// it; Apache-2.0 (§4), MIT and BSD all require the notice to accompany a redistribution.
//
//   node scripts/third-party-licenses.mjs            write the file
//   node scripts/third-party-licenses.mjs --check    also fail if a production package ships no licence text
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const lock = JSON.parse(readFileSync(join(root, 'package-lock.json'), 'utf8'))
const check = process.argv.includes('--check')
const LICENSE_FILE = /^(licen[cs]e|notice|copying|unlicense)(\.|$|-)/i

const out = []
const missing = []
const names = Object.keys(lock.packages)
  .filter((p) => p.startsWith('node_modules/') && !lock.packages[p].dev && !lock.packages[p].optional)
  .sort()
for (const p of names) {
  const dir = join(root, p)
  if (!existsSync(dir)) continue
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
  const files = readdirSync(dir).filter((f) => LICENSE_FILE.test(f)).sort()
  out.push(`${'='.repeat(78)}\n${pkg.name}@${pkg.version}  (${typeof pkg.license === 'string' ? pkg.license : JSON.stringify(pkg.license)})\n${'='.repeat(78)}`)
  if (files.length === 0) missing.push(`${pkg.name}@${pkg.version}`)
  for (const f of files) out.push(`--- ${f} ---\n${readFileSync(join(dir, f), 'utf8').trim()}\n`)
  if (files.length === 0) out.push(`(no licence file in the package; declared licence: ${pkg.license ?? 'unknown'})\n`)
}
mkdirSync(join(root, 'dist'), { recursive: true })
writeFileSync(join(root, 'dist', 'THIRD_PARTY_LICENSES'), `${out.join('\n')}\n`)
console.log(`THIRD_PARTY_LICENSES: ${names.length} production packages, ${missing.length} without a licence file`)
if (missing.length) console.log(`  no licence file: ${missing.join(', ')}`)
if (check && !names.some((n) => n === 'node_modules/@mysten/sui')) {
  console.error('expected @mysten/sui among the production packages')
  process.exit(1)
}
