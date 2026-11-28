import * as v from 'valibot'

import { Outcome } from './outcome'
import { Shares } from './shares'

const NoticeSchema = v.variant('action', [
  // `null` for a thread atuin hasn't captured.
  v.object({ action: v.literal('private'), outcome: Outcome.schema(v.nullable(Shares.schema)) }),
  v.object({
    action: v.literal('link'),
    outcome: Outcome.schema(v.object({ link: v.string(), isCopied: v.boolean() })),
  }),
])

/** The last action pressed in the shares pane, and how it went. */
export type Notice = v.InferOutput<typeof NoticeSchema>

const schema = v.object({ notice: v.nullable(NoticeSchema), isConfirmingPrivate: v.boolean() })

/** What the shares pane shows besides the report: the last action's outcome. */
export type SharesPane = v.InferOutput<typeof schema>

export const SharesPane = { schema }
