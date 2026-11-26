import type { Harness } from '@atuin/modkit'

import type { Capture, ShareState, ThreadShares, ThreadStatus, Upload, Workspace } from './types'
import type { Config } from './config'

/** What a call of the atuin CLI came to: its value, or why there is none. */
export type Outcome<T> = { isOk: true; value: T } | { isOk: false; reason: string }

const CLI_TIMEOUT_MS = 10_000

const CAPTURES: readonly Capture[] = ['on', 'off']
const UPLOADS: readonly Upload[] = ['on', 'off', 'logged-out']
const STATES: readonly ShareState[] = ['opening', 'open', 'closing', 'closed']

const failed = <T>(reason: string): Outcome<T> => ({ isOk: false, reason })

/** Runs `atuin <args>`, failing soft: a missing binary or an error exit is a reason, never a throw. */
export async function runAtuin(
  harness: Harness,
  { config, args }: { config: Config; args: readonly string[] },
): Promise<Outcome<string>> {
  try {
    const run = await harness.run([config.atuinPath, ...args], { timeoutMs: CLI_TIMEOUT_MS })
    if (run.exitCode === 0) return { isOk: true, value: run.stdout }
    const firstLine = run.stderr.trim().split('\n')[0] ?? ''
    return failed(firstLine === '' ? `atuin exited ${run.exitCode ?? 'without a code'}` : firstLine)
  } catch (err) {
    return failed(`could not run ${config.atuinPath}: ${String(err)}`)
  }
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

const isStrings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every(item => typeof item === 'string')

const parseJson = (text: string): unknown => {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

/** Reads `threads status --style json`, or undefined for any other shape. */
export const parseStatus = (text: string): ThreadStatus | undefined => {
  const json = parseJson(text)
  if (!isRecord(json)) return undefined
  const capture = CAPTURES.find(one => one === json.capture)
  if (capture === undefined) return undefined
  if (json.thread === null) return { capture, thread: null, octavo: null }
  if (!isRecord(json.thread) || typeof json.thread.messages !== 'number') return undefined
  const octavo = sharesOf(json.octavo)
  if (octavo === undefined) return undefined
  const updatedAt = typeof json.thread.updated_at === 'string' ? json.thread.updated_at : null
  return { capture, thread: { messages: json.thread.messages, updatedAt }, octavo }
}

const sharesOf = (json: unknown): ThreadShares | undefined => {
  if (!isRecord(json) || !Array.isArray(json.orgs) || !Array.isArray(json.dirs)) return undefined
  const upload = UPLOADS.find(one => one === json.upload)
  if (upload === undefined) return undefined
  const orgs = json.orgs.flatMap(org => {
    if (!isRecord(org) || typeof org.slug !== 'string') return []
    const state = STATES.find(one => one === org.state)
    return state === undefined ? [] : [{ slug: org.slug, state }]
  })
  const dirs = json.dirs.flatMap(dir =>
    isRecord(dir) && typeof dir.dir === 'string' && isStrings(dir.orgs)
      ? [{ dir: dir.dir, orgs: dir.orgs }]
      : [],
  )
  return { upload, orgs, dirs, isPrivate: json.private === true }
}

/** Reads `workspace orgs --style json`, or undefined for any other shape. */
export const parseWorkspace = (text: string): Workspace | undefined => {
  const json = parseJson(text)
  if (!isRecord(json) || !isStrings(json.orgs)) return undefined
  if (json.config !== null && typeof json.config !== 'string') return undefined
  return { orgs: json.orgs, config: json.config }
}

const parsed = <T>(
  outcome: Outcome<string>,
  { parse, what }: { parse: (text: string) => T | undefined; what: string },
): Outcome<T> => {
  if (!outcome.isOk) return outcome
  const value = parse(outcome.value)
  return value === undefined ? failed(`atuin printed no ${what}`) : { isOk: true, value }
}

/** Whether the thread `id` is captured, and who it's shared with as the daemon's queue has it. */
export async function threadStatus(
  harness: Harness,
  { config, id }: { config: Config; id: string },
): Promise<Outcome<ThreadStatus>> {
  const outcome = await runAtuin(harness, { config, args: ['threads', 'status', id, '--style', 'json'] })
  return parsed(outcome, { parse: parseStatus, what: 'status' })
}

/** The organizations `dir`'s config shares threads with, and the file that says so. */
export async function workspaceOrgs(
  harness: Harness,
  { config, dir }: { config: Config; dir: string },
): Promise<Outcome<Workspace>> {
  const outcome = await runAtuin(harness, { config, args: ['workspace', 'orgs', dir, '--style', 'json'] })
  return parsed(outcome, { parse: parseWorkspace, what: 'workspace' })
}
