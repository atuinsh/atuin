import { query } from '@atuin/modkit/ui/hooks'
import * as v from 'valibot'

import { threadStatus } from './cli'
import type { Config } from './config'
import { reportOf } from './status'
import type { StatusReport } from './types'

const REPORT_EVERY_MS = 30_000

const SharesSchema = v.object({
  upload: v.picklist(['on', 'off', 'logged-out']),
  orgs: v.array(
    v.object({ slug: v.string(), state: v.picklist(['opening', 'open', 'closing', 'closed']) }),
  ),
  dirs: v.array(v.object({ dir: v.string(), orgs: v.array(v.string()) })),
  isPrivate: v.boolean(),
})

const ReportSchema: v.GenericSchema<StatusReport | null> = v.nullable(
  v.variant('kind', [
    v.object({
      kind: v.literal('known'),
      status: v.object({
        capture: v.picklist(['on', 'off']),
        thread: v.nullable(v.object({ messages: v.number(), updatedAt: v.nullable(v.string()) })),
        octavo: v.nullable(SharesSchema),
      }),
    }),
    v.object({ kind: v.literal('unavailable'), reason: v.string() }),
  ]),
)

export const report = query({
  id: 'report',
  schema: ReportSchema,
  initial: null,
  everyMs: REPORT_EVERY_MS,
  on: ['session.start', 'turn.end', 'shell.after'],
  fetch: async (harness, config: Config) =>
    reportOf(await threadStatus(harness, { config, id: harness.session.id })),
})
