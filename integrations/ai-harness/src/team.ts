import type { Harness } from '@atuin/modkit'
import * as v from 'valibot'

import { Activity } from './activity'
import type { Octavo } from './octavo'
import { Outcome } from './outcome'
import { TeamThread } from './team-thread'

/** How far back "working now" looks, and how many days the stats count. */
const ACTIVE_WITHIN_MS = 24 * 60 * 60 * 1000
const ACTIVITY_DAYS = 7

const orgSchema = v.object({
  org: v.string(),
  activity: Outcome.schema(Activity.schema),
  threads: Outcome.schema(v.array(TeamThread.schema)),
})

/** One of the user's organizations: what its members did, and their latest threads but the user's. */
export type OrgTeam = v.InferOutput<typeof orgSchema>

const schema = v.nullable(
  Outcome.schema(v.object({ username: v.nullable(v.string()), orgs: v.array(orgSchema) })),
)

/** The user's organizations on Octavo, before the first look and after it. */
export type Team = v.InferOutput<typeof schema>

export const Team = {
  schema,

  /** How many teammates' threads "working now" lists, one per digit key. */
  WORKING_ROWS: 5,
}

/** The time zone days start in for the person, else UTC. */
const timeZone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

export class TeamLookup {
  readonly #harness: Harness
  readonly #octavo: Octavo

  constructor({ harness, octavo }: { harness: Harness; octavo: Octavo }) {
    this.#harness = harness
    this.#octavo = octavo
  }

  /** What the user's organizations did lately, and what their members work on now. */
  async team(): Promise<Team> {
    const orgs = await this.#octavo.orgs()
    if (!orgs.isOk) return orgs
    const { username } = orgs.value
    const activeAfter = new Date(this.#harness.now() - ACTIVE_WITHIN_MS).toISOString()
    const found = await Promise.all(
      orgs.value.orgs.map(({ slug }) => this.#org({ org: slug, username, activeAfter })),
    )
    return Outcome.ok({ username, orgs: found })
  }

  async #org({
    org,
    username,
    activeAfter,
  }: {
    org: string
    username: string | null
    activeAfter: string
  }): Promise<OrgTeam> {
    const [threads, activity] = await Promise.all([
      this.#octavo.recentThreads({
        org,
        activeAfter,
        enough: read => TeamThread.others(read, username).length >= Team.WORKING_ROWS,
      }),
      this.#octavo.activity({ org, days: ACTIVITY_DAYS, timeZone: timeZone() }),
    ])
    return {
      org,
      activity,
      threads: threads.isOk ? Outcome.ok(TeamThread.others(threads.value, username)) : threads,
    }
  }
}
