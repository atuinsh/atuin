import type { Harness } from '@atuin/modkit'
import * as v from 'valibot'

import { Activity } from './activity'
import { Conversation } from './conversation'
import { Outcome } from './outcome'
import { SeenHit } from './seen-hit'
import { TeamThread } from './team-thread'

const DEFAULT_SERVER = 'atuin'
const THREADS_PAGE = 50
const THREADS_PAGES = 5
const SEARCH_HITS = 3
const CONVERSATION_MESSAGES = 100

const OrgsSchema = v.object({
  username: v.nullish(v.string(), null),
  orgs: v.array(v.object({ slug: v.string() })),
})

/** The caller's username and their organizations with Octavo on. */
export type Orgs = v.InferOutput<typeof OrgsSchema>

const ThreadsSchema = v.object({
  threads: v.array(TeamThread.schema),
  next_cursor: v.nullish(v.string(), null),
})

const HitsSchema = v.object({ hits: v.array(SeenHit.schema) })

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export type OctavoSettings = { mcpServer: string }

export class Octavo {
  readonly settings: OctavoSettings
  readonly #harness: Harness

  constructor(harness: Harness, { mcpServer }: OctavoSettings) {
    this.#harness = harness
    // A setting left blank reads as its default.
    this.settings = { mcpServer: mcpServer.trim() || DEFAULT_SERVER }
  }

  async orgs(): Promise<Outcome<Orgs>> {
    return this.#call('list_orgs', { schema: OrgsSchema, what: 'organizations' })
  }

  /**
   * The threads shared with `org` active since `activeAfter`, the most recently active first, read
   * a page at a time until `enough` says it has seen what it needs.
   */
  async recentThreads({
    org,
    activeAfter,
    enough,
  }: {
    org: string
    activeAfter: string
    enough: (threads: readonly TeamThread[]) => boolean
  }): Promise<Outcome<TeamThread[]>> {
    const threads: TeamThread[] = []
    let cursor: string | null = null
    for (let page = 0; page < THREADS_PAGES; page += 1) {
      const answer: Outcome<v.InferOutput<typeof ThreadsSchema>> = await this.#call('list_recent_threads', {
        input: { org_slug: org, active_after: activeAfter, limit: THREADS_PAGE, ...(cursor !== null && { cursor }) },
        schema: ThreadsSchema,
        what: 'threads',
      })
      // A later page that fails leaves the pages read before it.
      if (!answer.isOk) return page === 0 ? answer : Outcome.ok(threads)
      threads.push(...answer.value.threads)
      cursor = answer.value.next_cursor
      if (cursor === null || cursor === '' || enough(threads)) break
    }
    return Outcome.ok(threads)
  }

  /** What `org`'s members did with agents over the last `days` days, by day in `timeZone`. */
  async activity({
    org,
    days,
    timeZone,
  }: {
    org: string
    days: number
    timeZone: string
  }): Promise<Outcome<Activity>> {
    return this.#call('get_activity', {
      input: { org_slug: org, days, time_zone: timeZone },
      schema: Activity.schema,
      what: 'activity',
    })
  }

  /** The threads shared with `org`, or the caller's own when `org` is null, matching `query`. */
  async search({ org, query }: { org: string | null; query: string }): Promise<Outcome<SeenHit[]>> {
    const answer = await this.#call('search_threads', {
      input: { query, limit: SEARCH_HITS, ...(org !== null && { org_slug: org }) },
      schema: HitsSchema,
      what: 'hits',
    })
    return answer.isOk ? Outcome.ok(answer.value.hits) : answer
  }

  async conversation(shareId: string): Promise<Outcome<Conversation>> {
    return this.#call('get_thread', {
      input: { share_id: shareId, detail: 'conversation', message_limit: CONVERSATION_MESSAGES },
      schema: Conversation.schema,
      what: 'conversation',
    })
  }

  /** Calls `tool` on the Octavo MCP server, failing soft. */
  async #call<S extends v.GenericSchema>(
    tool: string,
    { input, schema, what }: { input?: Record<string, unknown>; schema: S; what: string },
  ): Promise<Outcome<v.InferOutput<S>>> {
    const { mcpServer } = this.settings
    const result = await this.#harness.mcp({ server: mcpServer, tool, ...(input !== undefined && { input }) })
    if (!result.isOk) return Outcome.failed(`${mcpServer} ${tool}: ${result.reason}`)
    // The tool's JSON answer is its structured content, else its text parsed.
    const answer = isRecord(result.structured)
      ? v.safeParse(schema, result.structured)
      : v.safeParse(v.pipe(v.string(), v.parseJson(), schema), result.text)
    return answer.success ? Outcome.ok(answer.output) : Outcome.failed(`${mcpServer} ${tool} answered no ${what}`)
  }
}
