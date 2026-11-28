import { query, state } from '@atuin/modkit/ui/hooks'
import * as v from 'valibot'

import { Atuin, type AtuinSettings } from './atuin'
import { Conversation } from './conversation'
import { LocalThread } from './local-thread'
import { Octavo, type OctavoSettings } from './octavo'
import { Outcome } from './outcome'
import { SeenCard } from './seen'
import { SharesPane } from './shares-pane'
import { Report } from './status'
import { Team, TeamLookup } from './team'
import { Viewed, type Source } from './viewed'

const REPORT_EVERY_MS = 30_000
const TEAM_EVERY_MS = 60_000
const LOCAL_EVERY_MS = 60_000

/** How far back the pane looks at this machine's threads: the week its stats count. */
export const LOCAL_WITHIN_MS = 7 * 24 * 60 * 60 * 1000

/** How many of the project's earlier threads the pane lists. */
export const EARLIER_THREADS = 5

const LocalThreadsSchema = v.nullable(Outcome.schema(v.array(LocalThread.schema)))

export const report = query({
  id: 'report',
  schema: Report.schema,
  initial: null,
  everyMs: REPORT_EVERY_MS,
  on: ['session.start', 'turn.end', 'shell.after'],
  fetch: async (harness, settings: AtuinSettings) => new Atuin(harness, settings).threadStatus(),
})

// Asked for only while the pane is open, so the team is looked up then and never else.
export const team = query({
  id: 'team',
  schema: Team.schema,
  initial: null,
  everyMs: TEAM_EVERY_MS,
  fetch: async (harness, settings: OctavoSettings) =>
    new TeamLookup({ harness, octavo: new Octavo(harness, settings) }).team(),
})

// Asked for only while the pane is open: this machine's threads active in the last week.
export const recentThreads = query({
  id: 'recentThreads',
  schema: LocalThreadsSchema,
  initial: null,
  everyMs: LOCAL_EVERY_MS,
  fetch: async (harness, settings: AtuinSettings) => {
    const since = new Date(harness.now() - LOCAL_WITHIN_MS).toISOString()
    return new Atuin(harness, settings).threads({ since })
  },
})

// Asked for only while the pane is open: the session's project's newest threads, enough to list
// EARLIER_THREADS after leaving out the session's own and those under "Working now".
export const earlierThreads = query({
  id: 'earlierThreads',
  schema: LocalThreadsSchema,
  initial: null,
  everyMs: LOCAL_EVERY_MS,
  fetch: async (harness, settings: AtuinSettings) =>
    new Atuin(harness, settings).threads({
      workspace: await harness.root(),
      limit: EARLIER_THREADS + 1 + Team.WORKING_ROWS,
    }),
})

// A query rather than state, so a read cut short by a restart is asked for again.
export const conversation = query({
  id: 'conversation',
  schema: v.nullable(Outcome.schema(Conversation.schema)),
  initial: null,
  fetch: async (
    harness,
    { octavo, atuin, source }: { octavo: OctavoSettings; atuin: AtuinSettings; source: Source },
  ) =>
    source.kind === 'team'
      ? new Octavo(harness, octavo).conversation(source.thread.share_id)
      : new Atuin(harness, atuin).conversation(source.thread.atuin_id),
})

export const paneOpen = state({ id: 'paneOpen', schema: v.boolean(), initial: false })

export const sharesPane = state({
  id: 'sharesPane',
  schema: SharesPane.schema,
  initial: { notice: null, isConfirmingPrivate: false },
})

export const viewedThread = state({ id: 'viewedThread', schema: v.nullable(Viewed.schema), initial: null })

export const searchedErrors = state({ id: 'searchedErrors', schema: v.array(v.string()), initial: [] })

export const seenCards = state({ id: 'seenCards', schema: v.record(v.string(), SeenCard.schema), initial: {} })
