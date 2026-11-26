import { query, state } from '@atuin/modkit/ui/hooks'
import * as v from 'valibot'

import { Atuin, type AtuinSettings } from './atuin'
import { Conversation } from './conversation'
import { Octavo, type OctavoSettings } from './octavo'
import { SeenCard } from './seen'
import { SharesPane } from './shares-pane'
import { Report } from './status'
import { Team, TeamLookup } from './team'
import type { TeamThread } from './team-thread'
import { Viewed } from './viewed'

const REPORT_EVERY_MS = 30_000
const TEAM_EVERY_MS = 60_000

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

// A query rather than state, so a read cut short by a restart is asked for again.
export const conversation = query({
  id: 'conversation',
  schema: v.nullable(v.string()),
  initial: null,
  fetch: async (harness, { settings, thread }: { settings: OctavoSettings; thread: TeamThread }) => {
    const read = await new Octavo(harness, settings).conversation(thread.share_id)
    return read.isOk ? Conversation.markdown(read.value, thread) : `Could not read it: ${read.reason}`
  },
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
