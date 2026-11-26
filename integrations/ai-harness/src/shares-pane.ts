import * as v from 'valibot'

const schema = v.object({ notice: v.nullable(v.string()), isConfirmingPrivate: v.boolean() })

/** What the shares pane shows besides the report: the last action's outcome. */
export type SharesPane = v.InferOutput<typeof schema>

export const SharesPane = { schema }
