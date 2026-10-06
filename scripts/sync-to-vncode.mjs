#!/usr/bin/env node
/**
 * sync-to-vncode.mjs — vendor this package into the vncode plugin pack.
 *
 * WHY A COPY AND NOT A SUBMODULE OR A DEPENDENCY
 * ---------------------------------------------
 * `text-to-sc` is the canonical source of `dsh-supercollider`. vncode is the
 * pack that ships it, and the pack has two properties that rule out every other
 * way of getting one into the other:
 *
 *   1. **A distribution ships `packages/` as a live-linked directory.** It is not
 *      an install step; the folder IS the application. So the plugin has to be a
 *      real folder under `packages/` — not a symlink out of the tree, which a
 *      zip carries as a dangling link, and not a `node_modules` entry, because
 *      `scripts/dist-manifest.txt` skips every `node_modules` outright.
 *   2. **`.dsh-version.json` records every package's version**, and
 *      `scripts/checks/check-node-routes.mjs` fails when a bundle's own
 *      version disagrees with it. A git submodule at an external path satisfies
 *      neither.
 *
 * So this copies, and the copy is what vncode ships. Run it after changing
 * anything under `plugin/`:
 *
 *     node scripts/sync-to-vncode.mjs                    # into ../dsh-plugins
 *     node scripts/sync-to-vncode.mjs --target <path>    # into another checkout
 *     node scripts/sync-to-vncode.mjs --check            # fail if it is stale
 *
 * `--check` is the one worth wiring into a habit: it is what tells you the copy
 * in vncode is behind the source here without writing anything.
 */

import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const repo = path.resolve(here, '..')
const source = path.join(repo, 'plugin')

/** Everything that must travel. Anything else in the folder is build debris. */
const INCLUDE = ['package.json', 'cordis.patch.yml', 'README.md', 'lib', 'skills', 'mcp']

/** Never copied: this is a checkout's own scratch, not the package. */
const SKIP_NAMES = new Set(['.git', 'node_modules', '.DS_Store', 'Thumbs.db', '.scratch', 'tools'])

function parseArgs(argv) {
  const options = { target: path.resolve(repo, '..', 'dsh-plugins'), check: false, quiet: false }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--check') options.check = true
    else if (arg === '--quiet') options.quiet = true
    else if (arg === '--target') {
      options.target = path.resolve(argv[index + 1] ?? '')
      index += 1
    } else if (arg === '--help' || arg === '-h') {
      console.log('usage: node scripts/sync-to-vncode.mjs [--target <vncode checkout>] [--check] [--quiet]')
      process.exit(0)
    }
  }
  return options
}

/** Every file under one root, as paths relative to it, sorted. */
function listFiles(root, relative = '') {
  const found = []
  for (const entry of readdirSync(path.join(root, relative), { withFileTypes: true })) {
    if (SKIP_NAMES.has(entry.name)) continue
    const next = relative === '' ? entry.name : relative + '/' + entry.name
    if (entry.isDirectory()) found.push(...listFiles(root, next))
    else if (entry.isFile()) found.push(next)
  }
  return found.sort()
}

/** The files this sync is responsible for, from the include list. */
function listSource() {
  const files = []
  for (const entry of INCLUDE) {
    const full = path.join(source, entry)
    if (!existsSync(full)) continue
    const stats = statSync(full)
    if (stats.isDirectory()) {
      for (const file of listFiles(full)) files.push(entry + '/' + file)
    } else {
      files.push(entry)
    }
  }
  return files.sort()
}

function readOrNull(file) {
  try {
    return readFileSync(file)
  } catch (err) {
    return null
  }
}

function main() {
  const options = parseArgs(process.argv.slice(2))
  const destination = path.join(options.target, 'packages', 'dsh-supercollider')
  const say = (text) => {
    if (!options.quiet) console.log(text)
  }

  if (!existsSync(source)) {
    console.error('sync-to-vncode: there is no plugin/ folder in ' + repo)
    process.exit(1)
  }
  if (!existsSync(options.target)) {
    console.error('sync-to-vncode: no vncode checkout at ' + options.target + '\n  pass --target <path> to point at one.')
    process.exit(1)
  }

  const wanted = listSource()
  const differences = []
  for (const relative of wanted) {
    const from = path.join(source, relative)
    const to = path.join(destination, relative)
    const a = readOrNull(from)
    const b = readOrNull(to)
    if (a === null) continue
    if (b === null || !a.equals(b)) differences.push(relative)
  }

  if (options.check) {
    if (differences.length === 0) {
      say('sync-to-vncode: the copy in ' + destination + ' matches this source (' + wanted.length + ' files)')
      process.exit(0)
    }
    console.error('sync-to-vncode: ' + differences.length + ' file(s) differ from this source:')
    for (const relative of differences.slice(0, 40)) console.error('  ' + relative)
    if (differences.length > 40) console.error('  … and ' + (differences.length - 40) + ' more')
    console.error('\n  run: node scripts/sync-to-vncode.mjs --target ' + options.target)
    process.exit(1)
  }

  // Replace the copy outright rather than merging into it: a file REMOVED here
  // must disappear there, or the pack ships something this repository no longer
  // has any reason to contain.
  rmSync(destination, { recursive: true, force: true })
  mkdirSync(destination, { recursive: true })
  for (const relative of wanted) {
    const to = path.join(destination, relative)
    mkdirSync(path.dirname(to), { recursive: true })
    writeFileSync(to, readFileSync(path.join(source, relative)))
  }

  say('sync-to-vncode: wrote ' + wanted.length + ' file(s) to ' + destination)
  if (differences.length > 0) say('  (' + differences.length + ' replaced)')
  say('')
  say('Next, in the vncode checkout:')
  say('  1. add a "dsh-supercollider" entry to .dsh-version.json (rows: ["supercollider"])')
  say('  2. add a row for it to README.md\'s plugin table')
  say('  3. run: node scripts/checks/check-node-routes.mjs')
  return 0
}

process.exitCode = main()
