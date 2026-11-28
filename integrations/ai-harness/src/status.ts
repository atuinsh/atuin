import * as v from 'valibot'

import { Outcome } from './outcome'
import { Shares } from './shares'

const SYNCS = ['on', 'off', 'logged-out'] as const

/** Whether atuin sync takes the user's threads to their other machines. */
export type Sync = (typeof SYNCS)[number]

const schema = v.object({
  capture: v.picklist(['on', 'off']),
  // An atuin that can't say prints none.
  sync: v.optional(v.picklist(SYNCS)),
  thread: v.nullable(v.object({ messages: v.number() })),
  octavo: v.nullable(Shares.schema),
})

/** What `atuin threads status <id> --style json` prints: `thread` and `octavo` null until captured. */
export type ThreadStatus = v.InferOutput<typeof schema>

export const ThreadStatus = { schema }

/** The thread's status, or why the CLI could not say. */
export type Report = Outcome<ThreadStatus> | null

export const Report = {
  schema: v.nullable(Outcome.schema(schema)),

  /** Who the thread is shared with on Octavo, if atuin captured it and the CLI said. */
  shares: (report: Report): Shares | null => (report?.isOk === true ? report.value.octavo : null),
}
