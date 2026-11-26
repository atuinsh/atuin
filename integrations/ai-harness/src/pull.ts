import type { Harness } from '@atuin/modkit'

import { Conversation } from './conversation'
import type { Octavo } from './octavo'
import { Outcome } from './outcome'
import type { TeamThread } from './team-thread'

const DEFAULT_MODEL = 'haiku'
const SUMMARY_TIMEOUT_MS = 60_000

const SYSTEM = `You summarise a teammate's coding agent thread for another agent picking up related work.
Say what they set out to do, what they found and decided, what changed (files, commands, PRs), and
what was left open. Plain sentences and short bullets, under 250 words. No preamble.`

export class Pull {
  readonly #harness: Harness
  readonly #octavo: Octavo
  readonly #model: string

  constructor({ harness, octavo, summaryModel }: { harness: Harness; octavo: Octavo; summaryModel: string }) {
    this.#harness = harness
    this.#octavo = octavo
    // A setting left blank reads as its default.
    this.#model = summaryModel.trim() || DEFAULT_MODEL
  }

  /** Summarises `thread` and appends the summary to this session for the model to read. */
  async of({ org, thread }: { org: string; thread: TeamThread }): Promise<Outcome<string>> {
    const conversation = await this.#octavo.conversation(thread.share_id)
    if (!conversation.isOk) return conversation
    const transcript = Conversation.transcript(conversation.value)
    if (transcript === '') return Outcome.failed('the thread has no text yet')
    const summary = await this.#harness.complete({
      model: this.#model,
      system: SYSTEM,
      prompt: transcript,
      effort: 'low',
      maxTokens: 1024,
      timeoutMs: SUMMARY_TIMEOUT_MS,
    })
    if (!summary.isAnswered) return Outcome.failed(`the summary failed (${summary.detail})`)
    const note = this.#note({ org, thread, summary: summary.text.trim() })
    if ((await this.#harness.inject(note)) === 'dropped') return Outcome.failed('the session refused the summary')
    return Outcome.ok(note)
  }

  /** The row a pulled thread leaves the model. */
  #note({ org, thread, summary }: { org: string; thread: TeamThread; summary: string }): string {
    const branch = thread.git_branch === null ? '' : `, branch ${thread.git_branch}`
    return [
      `Context pulled in from ${thread.owner}'s Octavo thread "${thread.title}", shared with ${org}`,
      `(cwd ${thread.cwd}${branch}, share_id ${thread.share_id}).`,
      '',
      summary,
      '',
      `Read more of it with the ${this.#octavo.settings.mcpServer} MCP server's get_thread tool, passing share_id ${thread.share_id}.`,
    ].join('\n')
  }
}
