import type { Harness } from '@atuin/modkit'
import * as v from 'valibot'

import { Outcome } from './outcome'
import { ThreadStatus } from './status'

const CLI_TIMEOUT_MS = 10_000
const DEFAULT_PATH = 'atuin'

const WorkspaceSchema = v.object({ orgs: v.array(v.string()), config: v.nullable(v.string()) })

/** What `atuin workspace orgs <dir> --style json` prints: `config` null when no file decides. */
export type Workspace = v.InferOutput<typeof WorkspaceSchema>

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
