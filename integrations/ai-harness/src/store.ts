import { query } from '@atuin/modkit/ui/hooks'

import { Atuin, type AtuinSettings } from './atuin'
import { Report } from './status'

const REPORT_EVERY_MS = 30_000

export const report = query({
  id: 'report',
  schema: Report.schema,
  initial: null,
  everyMs: REPORT_EVERY_MS,
  on: ['session.start', 'turn.end', 'shell.after'],
  fetch: async (harness, settings: AtuinSettings) => new Atuin(harness, settings).threadStatus(),
})
