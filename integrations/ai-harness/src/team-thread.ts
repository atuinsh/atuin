import * as v from 'valibot'

// A row of `list_recent_threads` as the Octavo MCP server answers it, which leaves out what a
// thread lacks, so one schema reads the answer and the store.
const schema = v.pipe(
  v.object({
    share_id: v.string(),
    thread_id: v.string(),
    owner: v.nullish(v.string(), 'someone'),
    title: v.nullish(v.string()),
    preview: v.nullish(v.string()),
    cwd: v.nullish(v.string(), ''),
    git_branch: v.nullish(v.string(), null),
    last_active_at: v.nullish(v.string(), ''),
  }),
  v.transform(({ preview, ...row }) => ({ ...row, title: row.title ?? preview ?? 'Untitled thread' })),
)

/** A teammate's thread shared with one of the user's organizations. */
export type TeamThread = v.InferOutput<typeof schema>

export const TeamThread = {
  schema,

  /** The threads that are not `username`'s, each once, the most recently active first. */
  others: (threads: readonly TeamThread[], username: string | null): TeamThread[] => {
    const theirs = threads.filter(thread => thread.owner !== username)
    const unique = [...new Map(theirs.map(thread => [thread.thread_id, thread])).values()]
    return unique.sort((a, b) => b.last_active_at.localeCompare(a.last_active_at))
  },
}
