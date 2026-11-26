import * as v from 'valibot'

const CARD_HITS = 3

// A hit of `search_threads` as the Octavo MCP server answers it, which leaves out what a thread
// lacks, so one schema reads the answer and the store. Its start tells the session's own thread
// apart.
const schema = v.pipe(
  v.object({
    thread_id: v.string(),
    share_id: v.nullish(v.string(), null),
    owner: v.nullish(v.string(), null),
    title: v.nullish(v.string()),
    preview: v.nullish(v.string()),
    snippet: v.nullish(v.string(), ''),
    started_at: v.nullish(v.string(), null),
  }),
  v.transform(({ preview, ...hit }) => ({
    ...hit,
    title: hit.title ?? preview ?? 'Untitled thread',
    snippet: hit.snippet.replaceAll('**', '').replace(/\s+/g, ' ').trim(),
  })),
)

/** A thread that matched a failed command's error. */
export type SeenHit = v.InferOutput<typeof schema>

export const SeenHit = {
  schema,

  /** `hits` without the session's own thread, `startedAt` naming it, each thread once. */
  others: ({
    hits,
    startedAt,
    username,
  }: {
    hits: readonly SeenHit[]
    startedAt: string | undefined
    username: string | null
  }): SeenHit[] => {
    const sameInstant = (iso: string | null) =>
      iso !== null && startedAt !== undefined && Date.parse(iso) === Date.parse(startedAt)
    const others = hits.filter(hit => {
      const isMine = hit.owner === null || hit.owner === username
      return !(isMine && sameInstant(hit.started_at))
    })
    const unique = new Map(others.map(hit => [hit.thread_id, hit]))
    return [...unique.values()].slice(0, CARD_HITS)
  },
}
