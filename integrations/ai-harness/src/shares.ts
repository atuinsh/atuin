import * as v from 'valibot'

const STATES = ['opening', 'open', 'closing', 'closed'] as const

/** How far one of a thread's organizations got, as the daemon's queue has it. */
export type ShareState = (typeof STATES)[number]

// As `atuin threads status --style json` prints it, so one schema reads the CLI and the store.
const schema = v.object({
  upload: v.picklist(['on', 'off', 'logged-out']),
  orgs: v.array(v.object({ slug: v.string(), state: v.picklist(STATES) })),
  // Each directory the thread had a row uploaded from, with the orgs its config shared with.
  dirs: v.array(v.object({ dir: v.string(), orgs: v.array(v.string()) })),
  // Whether the user made the thread private, which no config shares again. An atuin that
  // can't make one private prints none.
  private: v.optional(v.boolean(), false),
})

/** Who a captured thread is shared with on Octavo. */
export type Shares = v.InferOutput<typeof schema>

export const Shares = {
  schema,

  orgs: (shares: Shares, ...states: readonly ShareState[]): string[] =>
    shares.orgs.filter(org => states.includes(org.state)).map(org => org.slug),

  /** The organizations the thread is open or opening to. */
  sharing: (shares: Shares): string[] => Shares.orgs(shares, 'open', 'opening'),

  /** The organizations the thread is shared with that `target`'s config leaves out. */
  droppedBy: (shares: Shares, target: readonly string[]): string[] => {
    const kept = new Set(target.map(org => org.toLowerCase()))
    return Shares.sharing(shares).filter(org => !kept.has(org.toLowerCase()))
  },
}
