import type { Tone } from '@atuin/modkit'
import * as v from 'valibot'

import { Outcome } from './outcome'
import { Shares } from './shares'

const schema = v.object({
  capture: v.picklist(['on', 'off']),
  thread: v.nullable(v.object({ messages: v.number() })),
  octavo: v.nullable(Shares.schema),
})

/** What `atuin threads status <id> --style json` prints: `thread` and `octavo` null until captured. */
export type ThreadStatus = v.InferOutput<typeof schema>

export const ThreadStatus = { schema }

/** The thread's status, or why the CLI could not say. */
export type Report = Outcome<ThreadStatus> | null

/** What the chip says beside its `atuin` mark. */
export type Words = { text: string; tone: Tone }

/** How many orgs the chip names before it counts the rest, as the footer has little room. */
const NAMED_ORGS = 2

const named = (orgs: readonly string[]) =>
  orgs.length <= NAMED_ORGS
    ? orgs.join(', ')
    : `${orgs.slice(0, NAMED_ORGS).join(', ')} +${orgs.length - NAMED_ORGS}`

export const Report = {
  schema: v.nullable(Outcome.schema(schema)),

  /** Who the thread is shared with on Octavo, if atuin captured it and the CLI said. */
  shares: (report: Report): Shares | null => (report?.isOk === true ? report.value.octavo : null),

  /** The chip for `report`: whether atuin captures the thread, then who it shares it with. */
  words: (report: Report): Words | undefined => {
    if (report === null) return undefined
    if (!report.isOk) return { text: 'unavailable', tone: 'dim' }
    const { capture, octavo } = report.value
    if (capture === 'off') return { text: 'capture off', tone: 'dim' }
    if (octavo === null) return { text: 'not captured yet', tone: 'dim' }
    if (octavo.upload === 'off') return { text: 'captured', tone: 'plain' }
    if (octavo.upload === 'logged-out') return { text: 'captured, logged out', tone: 'dim' }
    if (octavo.private) {
      // Private only once Octavo has closed each org: until then its members can still open it.
      const closing = Shares.orgs(octavo, 'closing')
      return closing.length === 0
        ? { text: 'private', tone: 'dim' }
        : { text: `private once closed to ${named(closing)}`, tone: 'warning' }
    }

    const open = Shares.orgs(octavo, 'open')
    const sharing = Shares.sharing(octavo)
    if (sharing.length > 0) {
      // Only queued so far: Octavo hasn't opened it to anyone yet.
      const verb = open.length === 0 ? 'sharing' : 'shared'
      return { text: `${verb} with ${named(sharing)}`, tone: 'plain' }
    }
    // A close still queued hasn't reached Octavo: the org's members can open the thread until it has.
    const closing = Shares.orgs(octavo, 'closing')
    const closed = Shares.orgs(octavo, 'closed')
    const closures = [
      ...(closing.length > 0 ? [`closing to ${named(closing)}`] : []),
      ...(closed.length > 0 ? [`closed to ${named(closed)}`] : []),
    ]
    if (closures.length > 0) return { text: closures.join('; '), tone: 'warning' }
    return { text: 'uploaded, not shared', tone: 'dim' }
  },
}
