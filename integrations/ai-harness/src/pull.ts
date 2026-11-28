import type { Harness } from '@atuin/modkit'

import { Conversation } from './conversation'
import { LocalThread } from './local-thread'
import { Outcome } from './outcome'
import type { Source } from './viewed'

const DEFAULT_MODEL = 'haiku'
const SUMMARY_TIMEOUT_MS = 60_000

const SYSTEM = `You summarise a coding agent thread for another agent picking up related work.
Say what it set out to do, what it found and decided, what changed (files, commands, PRs), and
what was left open. Plain sentences and short bullets, under 250 words. No preamble.`

export class Pull {
  readonly #harness: Harness
  readonly #model: string

  constructor({ harness, summaryModel }: { harness: Harness; summaryModel: string }) {
    this.#harness = harness
    // A setting left blank reads as its default.
    this.#model = summaryModel.trim() || DEFAULT_MODEL
  }

  /**
   * Summarises the thread `source` names, as `read` gives it, and appends the summary to this
   * session for the model to read, with `readMore`: how the model reads more of it.
   */
  async of({
    source,
    read,
    readMore,
  }: {
    source: Source
    read: () => Promise<Outcome<Conversation>>
    readMore: string
  }): Promise<Outcome<string>> {
    const conversation = await read()
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
    const note = [Pull.#from(source), '', summary.text.trim(), '', readMore].join('\n')
    if ((await this.#harness.inject(note)) === 'dropped') return Outcome.failed('the session refused the summary')
    return Outcome.ok(note)
  }

  /** Where a pulled thread came from, as the row it leaves the model starts. */
  static #from(source: Source): string {
    const { thread } = source
    const branch = thread.git_branch === null ? '' : `, branch ${thread.git_branch}`
    if (source.kind === 'team') {
      return [
        `Context pulled in from ${source.thread.owner}'s Octavo thread "${thread.title}", shared with ${source.org}`,
        `(cwd ${thread.cwd}${branch}, share_id ${source.thread.share_id}).`,
      ].join('\n')
    }
    const agent = LocalThread.agentName(source.thread.harness)
    return [
      `Context pulled in from your ${agent} thread "${thread.title}"`,
      `(cwd ${thread.cwd}${branch}, atuin id ${source.thread.atuin_id}).`,
    ].join('\n')
  }
}
