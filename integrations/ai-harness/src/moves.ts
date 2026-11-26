import type { Harness, ShellCall, ToolCall, Verdict } from '@atuin/modkit'

import type { Atuin } from './atuin'
import { Cd } from './cd'
import { Path } from './path'
import { Shares } from './shares'

export class Moves {
  readonly #harness: Harness
  readonly #atuin: Atuin
  readonly #shares: Shares | null

  constructor({ harness, atuin, shares }: { harness: Harness; atuin: Atuin; shares: Shares | null }) {
    this.#harness = harness
    this.#atuin = atuin
    this.#shares = shares
  }

  async ofShell({ command, cwd }: ShellCall): Promise<Verdict> {
    const written = Cd.leading(command)
    if (written === undefined) return undefined
    const dir = Path.resolve({ base: cwd, path: written, home: await this.#harness.env('HOME') })
    // A `cd` outside the project does not carry to the next call, so the thread never runs there.
    if (!Path.isWithin({ path: dir, root: await this.#harness.root() })) return undefined
    return this.#to(dir)
  }

  async ofWorktree({ input, cwd }: ToolCall): Promise<Verdict> {
    if (typeof input.path !== 'string') return undefined
    return this.#to(Path.resolve({ base: cwd, path: input.path }))
  }

  /**
   * Puts a move of the thread to `dir` to the person when it narrows the thread's shares. With
   * nobody to ask, it goes ahead, and Claude Code shows the reason: a warning is not worth blocking
   * a headless run.
   */
  async #to(dir: string): Promise<Verdict> {
    const shares = this.#shares
    if (shares === null || Shares.sharing(shares).length === 0) return undefined
    const looked = await this.#atuin.workspaceOrgs(dir)
    if (!looked.isOk) return undefined
    const dropped = Shares.droppedBy(shares, looked.value.orgs)
    if (dropped.length === 0) return undefined
    const whom = dropped.length === 1 ? 'it' : 'them'
    return {
      ask: `Running in ${dir} closes this Octavo thread to ${dropped.join(', ')} for good, as its config does not share with ${whom}.`,
      otherwise: 'allow',
    }
  }
}
