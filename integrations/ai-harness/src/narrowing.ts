import type { Harness, Verdict } from '@atuin/modkit'

import { workspaceOrgs } from './cli'
import type { Config } from './config'
import { sharingOrgs } from './status'
import type { ThreadShares } from './types'

const unquote = (word: string): string => {
  const quote = word[0]
  return (quote === '"' || quote === "'") && word.endsWith(quote) && word.length >= 2
    ? word.slice(1, -1)
    : word
}

/**
 * The directory a command's leading `cd` moves to, as written: `cd src && make` gives `src`.
 *
 * Undefined for a command that starts otherwise, and for `cd` alone, `cd -` or a target the shell
 * expands (`$VAR`, a backtick), whose directory depends on the shell's state.
 */
export const leadingCd = (command: string): string | undefined => {
  // `cd`'s own options (`-L`, `-P`, `-e`, `-@`) and the `--` that ends them come before the target.
  const match = /^\s*cd\s+(?:-[LPe@]+\s+)*(?:--\s+)?("[^"]*"|'[^']*'|[^\s;&|]+)\s*(?:$|&&|;|\|\||\n)/.exec(command)
  const target = match?.[1]
  if (target === undefined) return undefined
  const dir = unquote(target)
  return dir === '' || dir.startsWith('-') || /[$`]/.test(dir) ? undefined : dir
}

/** `path` resolved against `base`, `~` against `home`, with `.` and `..` folded. */
export const resolvePath = ({ base, path, home }: { base: string; path: string; home?: string }) => {
  const expanded =
    home !== undefined && (path === '~' || path.startsWith('~/')) ? `${home}${path.slice(1)}` : path
  const joined = expanded.startsWith('/') ? expanded : `${base}/${expanded}`
  const parts = joined.split('/').reduce<string[]>((kept, part) => {
    if (part === '' || part === '.') return kept
    if (part === '..') return kept.slice(0, -1)
    return [...kept, part]
  }, [])
  return `/${parts.join('/')}`
}

/** Whether `path` is `root` or inside it. */
export const isWithin = ({ path, root }: { path: string; root: string }) =>
  path === root || path.startsWith(root.endsWith('/') ? root : `${root}/`)

/** The organizations the thread is shared with that `target`'s config leaves out. */
export const droppedOrgs = ({ shares, target }: { shares: ThreadShares; target: readonly string[] }) => {
  const kept = new Set(target.map(org => org.toLowerCase()))
  return sharingOrgs(shares).filter(org => !kept.has(org.toLowerCase()))
}

/** Why moving to `dir` needs the person's word, naming what it closes the thread to. */
export const narrowingReason = ({ dir, dropped }: { dir: string; dropped: readonly string[] }) =>
  `Running in ${dir} closes this Octavo thread to ${dropped.join(', ')} for good, as its config does not share with ${dropped.length === 1 ? 'it' : 'them'}.`

/** The directory a shell command's leading `cd` moves the session to, inside the project only. */
export async function bashTarget(
  harness: Harness,
  { command, cwd }: { command: string; cwd: string },
): Promise<string | undefined> {
  const written = leadingCd(command)
  if (written === undefined) return undefined
  const dir = resolvePath({ base: cwd, path: written, home: await harness.env('HOME') })
  // A `cd` outside the project does not carry to the next call, so the thread never runs there.
  return isWithin({ path: dir, root: await harness.root() }) ? dir : undefined
}

/** The directory an `EnterWorktree` call moves the session to. */
export const worktreeTarget = ({ input, cwd }: { input: Readonly<Record<string, unknown>>; cwd: string }) =>
  typeof input.path === 'string' ? resolvePath({ base: cwd, path: input.path }) : undefined

/**
 * Puts a move of the thread to `dir` to the person when it narrows the thread's shares. With
 * nobody to ask, it goes ahead, and Claude Code shows the reason: a warning is not worth blocking
 * a headless run.
 */
export async function narrowing(
  harness: Harness,
  { config, shares, dir }: { config: Config; shares: ThreadShares | null; dir: string | undefined },
): Promise<Verdict> {
  if (dir === undefined || shares === null || sharingOrgs(shares).length === 0) return undefined
  const looked = await workspaceOrgs(harness, { config, dir })
  if (!looked.isOk) return undefined
  const dropped = droppedOrgs({ shares, target: looked.value.orgs })
  if (dropped.length === 0) return undefined
  return { ask: narrowingReason({ dir, dropped }), otherwise: 'allow' }
}
