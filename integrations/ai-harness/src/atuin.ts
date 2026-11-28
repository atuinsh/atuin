import type { Harness } from '@atuin/modkit'
import * as v from 'valibot'

import type { Conversation } from './conversation'
import { LocalThread } from './local-thread'
import { Outcome } from './outcome'
import { ThreadStatus } from './status'

const CLI_TIMEOUT_MS = 10_000
const DEFAULT_PATH = 'atuin'

const WorkspaceSchema = v.object({ orgs: v.array(v.string()), config: v.nullable(v.string()) })

/** What `atuin workspace orgs <dir> --style json` prints: `config` null when no file decides. */
export type Workspace = v.InferOutput<typeof WorkspaceSchema>

// A thread as `atuin threads list --style json` prints it, of which the mod reads its start.
const ListedSchema = v.array(v.object({ session_id: v.string(), started_at: v.nullish(v.string()) }))
type Listed = v.InferOutput<typeof ListedSchema>[number]

/** How far back the session's own thread is looked for first: it is active, so most likely captured then. */
const OWN_THREAD_WITHIN_MS = 24 * 60 * 60 * 1000

// A content block of a message, as `atuin threads show --style json` prints it, of which the mod
// reads the text.
const BlockSchema = v.looseObject({ type: v.string(), text: v.optional(v.string()) })

// A thread as `atuin threads show --style json` prints it, of which the mod reads what was said.
const ShownSchema = v.object({
  session: v.object({ last_reply: v.nullish(v.string(), null) }),
  messages: v.array(
    v.object({ role: v.string(), content: v.union([v.string(), v.array(BlockSchema)]) }),
  ),
})

/** The blocks whose text a reader reads: what was said, a compaction's recap, a recorded failure. */
const TEXT_BLOCKS: ReadonlySet<string> = new Set(['text', 'summary', 'error'])

/** The text of a message's `content`: its readable blocks, or the text itself. */
function textOf(content: string | readonly v.InferOutput<typeof BlockSchema>[]): string {
  if (typeof content === 'string') return content.trim() === '[]' ? '' : content
  return content
    .flatMap(block => (TEXT_BLOCKS.has(block.type) && block.text !== undefined ? [block.text] : []))
    .join('\n')
}

/** What the pane asks `atuin threads list` for: `undefined` leaves each out. */
export type ThreadsFilter = { workspace?: string; since?: string; limit?: number }

export type AtuinSettings = { atuinPath: string }

export class Atuin {
  readonly settings: AtuinSettings
  readonly #harness: Harness

  constructor(harness: Harness, { atuinPath }: AtuinSettings) {
    this.#harness = harness
    // A setting left blank reads as its default.
    this.settings = { atuinPath: atuinPath.trim() || DEFAULT_PATH }
  }

  /** Whether the session's thread is captured, and who it's shared with as the daemon's queue has it. */
  async threadStatus(): Promise<Outcome<ThreadStatus>> {
    return this.#json(['threads', 'status', this.#harness.session.id], {
      schema: ThreadStatus.schema,
      what: 'status',
    })
  }

  /** The organizations `dir`'s config shares threads with, and the file that says so. */
  async workspaceOrgs(dir: string): Promise<Outcome<Workspace>> {
    return this.#json(['workspace', 'orgs', dir], { schema: WorkspaceSchema, what: 'workspace' })
  }

  /** Closes the session's thread to every organization, now and from then on. */
  async unshare(): Promise<Outcome<ThreadStatus>> {
    return this.#json(['hub', 'unshare', this.#harness.session.id], {
      schema: ThreadStatus.schema,
      what: 'status',
    })
  }

  /** The session's thread's share link. */
  async share(): Promise<Outcome<string>> {
    const printed = await this.#run(['hub', 'share', this.#harness.session.id])
    if (!printed.isOk) return printed
    const link = printed.value
      .split('\n')
      .map(line => line.trim())
      .find(line => line !== '')
    return link === undefined ? Outcome.failed('atuin printed no link') : Outcome.ok(link)
  }

  /** When the session's thread started, as the local capture has it, if it says. */
  async startedAt(): Promise<Outcome<string>> {
    const since = new Date(this.#harness.now() - OWN_THREAD_WITHIN_MS).toISOString()
    const recent = await this.#listedOwn(['--since', since])
    // One resumed after capture turned off keeps its last capture's time, so every thread is
    // listed, slow as that is, when the recent ones lack it.
    const listed = recent.isOk && recent.value === undefined ? await this.#listedOwn([]) : recent
    if (!listed.isOk) return listed
    const started = listed.value?.started_at
    return typeof started === 'string' ? Outcome.ok(started) : Outcome.failed('atuin listed no start for this thread')
  }

  /**
   * The session's own thread among those `threads list` prints with `filter`, if listed. The
   * list, as `threads show` prints every message of the thread besides.
   */
  async #listedOwn(filter: readonly string[]): Promise<Outcome<Listed | undefined>> {
    const listed = await this.#json(['threads', 'list', ...filter], { schema: ListedSchema, what: 'threads' })
    if (!listed.isOk) return listed
    return Outcome.ok(listed.value.find(thread => thread.session_id === this.#harness.session.id))
  }

  /**
   * The threads agents on this machine captured that `filter` passes, newest first, without
   * subagents': those synced from the user's other machines are theirs.
   */
  async threads({ workspace, since, limit }: ThreadsFilter): Promise<Outcome<LocalThread[]>> {
    const args = [
      'threads',
      'list',
      '--this-host',
      '--exclude-subagents',
      ...(workspace === undefined ? [] : ['--workspace', workspace]),
      ...(since === undefined ? [] : ['--since', since]),
      ...(limit === undefined ? [] : ['--limit', String(limit)]),
    ]
    return this.#json(args, { schema: v.array(LocalThread.schema), what: 'threads' })
  }

  /** What was said in the thread whose atuin id is `id`, as a conversation. */
  async conversation(id: string): Promise<Outcome<Conversation>> {
    const shown = await this.#json(['threads', 'show', id], { schema: ShownSchema, what: 'thread' })
    if (!shown.isOk) return shown
    return Outcome.ok({
      messages: shown.value.messages.map(message => ({ role: message.role, text: textOf(message.content) })),
      next_cursor: null,
      last_reply: shown.value.session.last_reply,
    })
  }

  /**
   * The command that resumes the thread whose atuin id is `id`: in its own agent, or as a new session
   * of `agent`, which this writes out.
   */
  async resumeCommand({ id, agent }: { id: string; agent?: string }): Promise<Outcome<string>> {
    const printed = await this.#run(['threads', 'resume', id, ...(agent === undefined ? [] : ['--in', agent]), '--print'])
    if (!printed.isOk) return printed
    const command = printed.value.trim()
    return command === '' ? Outcome.failed('atuin printed no command') : Outcome.ok(command)
  }

  async #json<S extends v.GenericSchema>(
    args: readonly string[],
    { schema, what }: { schema: S; what: string },
  ): Promise<Outcome<v.InferOutput<S>>> {
    const run = await this.#run([...args, '--style', 'json'])
    if (!run.isOk) return run
    const printed = v.safeParse(v.pipe(v.string(), v.parseJson(), schema), run.value)
    return printed.success ? Outcome.ok(printed.output) : Outcome.failed(`atuin printed no ${what}`)
  }

  /** Runs `atuin <args>`, failing soft: a missing binary or an error exit is a reason, never a throw. */
  async #run(args: readonly string[]): Promise<Outcome<string>> {
    const { atuinPath } = this.settings
    try {
      const run = await this.#harness.run([atuinPath, ...args], { timeoutMs: CLI_TIMEOUT_MS })
      if (run.exitCode === 0) return Outcome.ok(run.stdout)
      const firstLine = run.stderr.trim().split('\n')[0] ?? ''
      return Outcome.failed(
        firstLine === '' ? `atuin exited ${run.exitCode ?? 'without a code'}` : firstLine,
      )
    } catch (err) {
      return Outcome.failed(`could not run ${atuinPath}: ${String(err)}`)
    }
  }
}
