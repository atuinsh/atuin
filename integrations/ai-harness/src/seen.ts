import * as v from 'valibot'

import type { Atuin } from './atuin'
import type { Octavo } from './octavo'
import { SeenHit } from './seen-hit'

const QUERY_WORDS = 8
const ERROR_WORDS = /error|fail|fatal|panic|exception|denied|refused|not found|cannot|can't|unable|invalid|missing|undefined/i

const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g

const schema = v.object({ query: v.string(), hits: v.array(SeenHit.schema), isAdded: v.boolean() })

/** The hits drawn under one failed Bash call. */
export type SeenCard = v.InferOutput<typeof schema>

export const SeenCard = {
  schema,

  /**
   * The words of a failed command's output worth searching for: its first line that reads as an
   * error, else its last, with paths, numbers and punctuation left out.
   */
  query: (output: string): string | undefined => {
    const lines = output
      .replace(ANSI, '')
      .split('\n')
      .map(line => line.trim())
      .filter(line => line !== '')
    const line = lines.find(one => ERROR_WORDS.test(one)) ?? lines.at(-1)
    if (line === undefined) return undefined
    const words = line
      .slice(0, 120)
      .replace(/\S*[/\\]\S*/g, ' ')
      .replace(/\b0x[0-9a-f]+\b|\b\d[\d.:]*\b/gi, ' ')
      .split(/[^A-Za-z_'-]+/)
      .filter(word => word.length >= 3)
      .slice(0, QUERY_WORDS)
    return words.length >= 2 ? words.join(' ') : undefined
  },

  /** The row that tells the model which threads hit the same error. */
  note: (card: SeenCard, mcpServer: string): string =>
    [
      `Octavo threads that hit the same error ("${card.query}"):`,
      ...card.hits.map(hit => {
        const ids = hit.share_id === null ? `thread_id ${hit.thread_id}` : `share_id ${hit.share_id}`
        return `- ${hit.owner ?? 'you'}: "${hit.title}" (${ids}): ${hit.snippet}`
      }),
      `Read one with the ${mcpServer} MCP server's get_thread tool.`,
    ].join('\n'),
}

export class SeenLookup {
  readonly #atuin: Atuin
  readonly #octavo: Octavo

  constructor({ atuin, octavo }: { atuin: Atuin; octavo: Octavo }) {
    this.#atuin = atuin
    this.#octavo = octavo
  }

  /** The threads of the user's organizations, and their own others, that hit `query` too. */
  async hits(query: string): Promise<SeenHit[]> {
    const orgs = await this.#octavo.orgs()
    if (!orgs.isOk) return []
    const started = await this.#atuin.startedAt()
    // Without its start the session's own thread can't be told from the user's others.
    const scopes = [...orgs.value.orgs.map(org => org.slug), ...(started.isOk ? [null] : [])]
    const found = await Promise.all(scopes.map(org => this.#octavo.search({ org, query })))
    return SeenHit.others({
      hits: found.flatMap(one => (one.isOk ? one.value : [])),
      startedAt: started.isOk ? started.value : undefined,
      username: orgs.value.username,
    })
  }
}
