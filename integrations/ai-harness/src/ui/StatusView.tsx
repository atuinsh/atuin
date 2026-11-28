import { Link, Text, type ModElement } from '@atuin/modkit'

import type { Outcome } from '../outcome'
import { Shares, type OrgShare } from '../shares'
import type { Sync, ThreadStatus } from '../status'

/** Atuin's green and the ink its logo draws on it, which reads there in light and dark themes. */
export const ATUIN_GREEN = '#26bd65'
const ATUIN_INK = '#263238'

/** How many orgs the status names before it counts the rest, as the footer has little room. */
const NAMED_ORGS = 2

const named = (orgs: readonly string[]) =>
  orgs.length <= NAMED_ORGS
    ? orgs.join(', ')
    : `${orgs.slice(0, NAMED_ORGS).join(', ')} +${orgs.length - NAMED_ORGS}`

/** Where atuin sync takes the thread. */
function SyncText({ sync }: { sync: Sync }): ModElement {
  switch (sync) {
    case 'on':
      return (
        <Text color={ATUIN_INK} background={ATUIN_GREEN} bold>
          {' Syncing '}
        </Text>
      )
    case 'off':
      return <Text tone="dim">Sync off</Text>
    case 'logged-out':
      return <Text tone="dim">Local</Text>
  }
}

/** `@slug` in bold, opening the org's page on Atuin Hub where the CLI said where that is. */
function OrgHandle({ org }: { org: OrgShare }) {
  const handle = <Text bold>@{org.slug}</Text>
  return org.url === undefined ? handle : <Link href={org.url}>{handle}</Link>
}

/** The orgs that can open the thread, a space apart, and a count of those past the first few. */
function SharedWith({ orgs }: { orgs: readonly OrgShare[] }) {
  return (
    <Text>
      {orgs.slice(0, NAMED_ORGS).map((org, index) => (
        <Text key={org.slug}>
          {index > 0 && ' '}
          <OrgHandle org={org} />
        </Text>
      ))}
      {orgs.length > NAMED_ORGS && <Text tone="dim"> +{orgs.length - NAMED_ORGS}</Text>}
    </Text>
  )
}

/** Who can open the thread on Atuin Hub, or nothing with the Hub off. */
function HubText({ shares }: { shares: Shares }) {
  if (shares.upload === 'off') return null
  if (shares.upload === 'logged-out') return <Text tone="warning">Logged out</Text>
  const closing = Shares.orgs(shares, 'closing')
  if (shares.private) {
    // Until Atuin Hub has closed each org and revoked each link, their holders can still open it.
    return closing.length > 0 || shares.revoking_links ? (
      <Text tone="warning">Going private</Text>
    ) : (
      <Text tone="dim">Private</Text>
    )
  }
  const sharing = shares.orgs.filter(org => org.state === 'open' || org.state === 'opening')
  if (Shares.orgs(shares, 'open').length > 0) return <SharedWith orgs={sharing} />
  // Only queued so far: Atuin Hub hasn't opened it to anyone yet.
  if (sharing.length > 0) return <Text>Sharing with {named(sharing.map(org => org.slug))}</Text>
  // A close still queued hasn't reached Atuin Hub: the org's members can open it until it has.
  if (closing.length > 0) return <Text tone="warning">Closing to {named(closing)}</Text>
  // No org sees it, but a share link, which `orgs` doesn't track, may still open it.
  return <Text tone="dim">Unlisted</Text>
}

/** Whether atuin syncs the thread, then who can open it on Atuin Hub. */
export function StatusView({ report }: { report: Outcome<ThreadStatus> }) {
  if (!report.isOk) return <Text tone="dim">Unavailable</Text>
  const { capture, sync, octavo } = report.value
  if (capture === 'off') return <Text tone="dim">Disabled</Text>
  // Before its first message the thread has no shares to show, but it syncs as the rest do.
  if (octavo === null) return sync === undefined ? null : <SyncText sync={sync} />
  // An atuin that doesn't say how it syncs still says it captured the thread.
  return (
    <Text>
      {sync === undefined ? <Text>Captured</Text> : <SyncText sync={sync} />}
      {octavo.upload !== 'off' && ' '}
      <HubText shares={octavo} />
    </Text>
  )
}
