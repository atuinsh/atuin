import * as v from 'valibot'

import { TeamThread } from './team-thread'

const schema = v.object({
  // The organization the thread is shared with, which its row doesn't name.
  org: v.string(),
  thread: TeamThread.schema,
  pull: v.picklist(['idle', 'pulling', 'pulled', 'failed']),
})

/** The thread the thread pane shows. */
export type Viewed = v.InferOutput<typeof schema>

export const Viewed = { schema }
