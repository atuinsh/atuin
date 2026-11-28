import * as v from 'valibot'

import { LocalThread } from './local-thread'
import { Outcome } from './outcome'
import { TeamThread } from './team-thread'

// Where the thread pane's thread comes from: a teammate's, shared with an org on Atuin Hub, or one
// an agent on this machine captured.
const SourceSchema = v.variant('kind', [
  // `org` is the organization the thread is shared with, which its row doesn't name.
  v.object({ kind: v.literal('team'), org: v.string(), thread: TeamThread.schema }),
  v.object({ kind: v.literal('local'), thread: LocalThread.schema }),
])

/** Where the thread pane's thread comes from, and the thread as its source lists it. */
export type Source = v.InferOutput<typeof SourceSchema>

const ResumeSchema = v.object({
  // The agent it was to resume in.
  agent: v.string(),
  outcome: Outcome.schema(v.object({ command: v.string(), isCopied: v.boolean() })),
})

/** The last resume pressed in the thread pane, and the command it gave, or why there is none. */
export type Resume = v.InferOutput<typeof ResumeSchema>

const schema = v.object({
  source: SourceSchema,
  pull: v.picklist(['idle', 'pulling', 'pulled', 'failed']),
  resume: v.nullable(ResumeSchema),
})

/** The thread the thread pane shows. */
export type Viewed = v.InferOutput<typeof schema>

export const Viewed = {
  schema,

  /** The thread `source` names, before anything was done with it. */
  of: (source: Source): Viewed => ({ source, pull: 'idle', resume: null }),

  /** What tells the thread apart from any other, across sources. */
  key: ({ source }: Viewed): string =>
    source.kind === 'team' ? `team:${source.thread.share_id}` : `local:${source.thread.atuin_id}`,

  /** Whose thread it is: a teammate, or the agent that ran it here. */
  owner: ({ source }: Viewed): string =>
    source.kind === 'team' ? source.thread.owner : LocalThread.agentName(source.thread.harness),
}
