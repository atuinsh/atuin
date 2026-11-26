import type { Tone as TextTone } from '@atuin/modkit'

import type { Outcome } from './cli'
import type { ShareState, StatusReport, ThreadShares, ThreadStatus } from './types'

/** How the chip's words read: what atuin does plainly, a closed share as a warning, the rest dim. */
export type Tone = 'plain' | 'warning' | 'quiet'

/** What the chip says beside its `atuin` mark. */
export type Chip = { text: string; tone: Tone }

/** The text tone that draws `tone`. */
export const toneOf = (tone: Tone): TextTone => (tone === 'quiet' ? 'dim' : tone)

/** How many orgs the chip names before it counts the rest, as the footer has little room. */
const NAMED_ORGS = 2

const orgsIn = (shares: ThreadShares, states: readonly ShareState[]) =>
  shares.orgs.filter(org => states.includes(org.state)).map(org => org.slug)

const named = (orgs: readonly string[]) =>
  orgs.length <= NAMED_ORGS
    ? orgs.join(', ')
    : `${orgs.slice(0, NAMED_ORGS).join(', ')} +${orgs.length - NAMED_ORGS}`

/** Who the thread is shared with on Octavo, if atuin captured it and the CLI said. */
export const octavoOf = (report: StatusReport | null): ThreadShares | null =>
  report?.kind === 'known' ? report.status.octavo : null

/** The chip for `report`: whether atuin captures the thread, then who it shares it with. */
export const chipOf = (report: StatusReport | null): Chip | undefined => {
  if (report === null) return undefined
  if (report.kind === 'unavailable') return { text: 'unavailable', tone: 'quiet' }
  const { capture, octavo } = report.status
  if (capture === 'off') return { text: 'capture off', tone: 'quiet' }
  if (octavo === null) return { text: 'not captured yet', tone: 'quiet' }
  if (octavo.upload === 'off') return { text: 'captured', tone: 'plain' }
  if (octavo.upload === 'logged-out') return { text: 'captured, logged out', tone: 'quiet' }
  if (octavo.isPrivate) {
    // Private only once Octavo has closed each org: until then its members can still open it.
    const closing = orgsIn(octavo, ['closing'])
    return closing.length === 0
      ? { text: 'private', tone: 'quiet' }
      : { text: `private once closed to ${named(closing)}`, tone: 'warning' }
  }

  const open = orgsIn(octavo, ['open'])
  const sharing = [...open, ...orgsIn(octavo, ['opening'])]
  if (sharing.length > 0) {
    // Only queued so far: Octavo hasn't opened it to anyone yet.
    const verb = open.length === 0 ? 'sharing' : 'shared'
    return { text: `${verb} with ${named(sharing)}`, tone: 'plain' }
  }
  // A close still queued hasn't reached Octavo: the org's members can open the thread until it has.
  const closing = orgsIn(octavo, ['closing'])
  const closed = orgsIn(octavo, ['closed'])
  const closures = [
    ...(closing.length > 0 ? [`closing to ${named(closing)}`] : []),
    ...(closed.length > 0 ? [`closed to ${named(closed)}`] : []),
  ]
  if (closures.length > 0) return { text: closures.join('; '), tone: 'warning' }
  return { text: 'uploaded, not shared', tone: 'quiet' }
}

/** The organizations the thread is open or opening to. */
export const sharingOrgs = (shares: ThreadShares): string[] => orgsIn(shares, ['open', 'opening'])

/** What a look at the thread came to, as the chip reads it. */
export const reportOf = (outcome: Outcome<ThreadStatus>): StatusReport =>
  outcome.isOk ? { kind: 'known', status: outcome.value } : { kind: 'unavailable', reason: outcome.reason }
