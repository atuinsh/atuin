import * as v from 'valibot'

const ACTIVE_MEMBERS = 4

// As `get_activity` answers for an organization, so one schema reads the answer and the store.
const schema = v.object({
  // Each day's threads, oldest first.
  days: v.array(v.object({ date: v.string(), threads: v.number() })),
  totals: v.object({
    threads: v.number(),
    tool_calls: v.number(),
    lines_added: v.number(),
    lines_removed: v.number(),
  }),
  members: v.array(v.object({ username: v.string(), threads: v.number() })),
})

/** What an organization's members did with agents over the last days, as `get_activity` counts. */
export type Activity = v.InferOutput<typeof schema>

export const Activity = {
  schema,

  /** The members with threads, the most first, the user as `you`: `ellie 69 · you 50`. */
  members: (activity: Activity, username: string | null): string =>
    activity.members
      .filter(member => member.threads > 0)
      .toSorted((a, b) => b.threads - a.threads)
      .slice(0, ACTIVE_MEMBERS)
      .map(member => `${member.username === username ? 'you' : member.username} ${member.threads}`)
      .join(' · '),
}
