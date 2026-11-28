import { Button, Column, Row, Text, type ModElement, type Press } from '@atuin/modkit'

import type { Activity } from '../activity'
import type { Outcome } from '../outcome'
import { Shares } from '../shares'
import type { Notice, SharesPane } from '../shares-pane'
import { Report } from '../status'
import { Team, type OrgTeam } from '../team'
import type { TeamThread } from '../team-thread'
import { ATUIN_GREEN, StatusView } from './StatusView'

const BARS = '▁▂▃▄▅▆▇█'
const ACTIVE_MEMBERS = 4

/** `values` as a row of bars, the tallest for the largest: `▁▄█`. */
const sparkline = (values: readonly number[]): string => {
  const most = Math.max(0, ...values)
  const top = BARS.length - 1
  return values.map(value => BARS[most === 0 ? 0 : Math.round((value / most) * top)]).join('')
}

/** `n` in a few characters: `950`, `15.2k`, `152k`, `1.3M`. */
const compact = (n: number): string => {
  const scaled = (value: number, unit: string) => `${value < 100 ? value.toFixed(1).replace(/\.0$/, '') : Math.round(value)}${unit}`
  if (n < 1000) return String(n)
  if (n < 1_000_000) return scaled(n / 1000, 'k')
  return scaled(n / 1_000_000, 'M')
}

/** `iso`'s age at `nowMs`, in the largest whole unit: `12m`, `3h`, `2d`. */
const age = ({ iso, nowMs }: { iso: string; nowMs: number }): string => {
  const minutes = Math.max(0, Math.floor((nowMs - Date.parse(iso)) / 60_000))
  if (Number.isNaN(minutes)) return '?'
  if (minutes < 60) return `${minutes}m`
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)}h`
  return `${Math.floor(minutes / (24 * 60))}d`
}

/** The members with threads, the most first, the user as `you`: `ellie 69 · you 50`. */
function MostActive({ activity, username }: { activity: Activity; username: string | null }) {
  const members = activity.members
    .filter(member => member.threads > 0)
    .toSorted((a, b) => b.threads - a.threads)
    .slice(0, ACTIVE_MEMBERS)
  if (members.length === 0) return null
  const named = members.map(member => `${member.username === username ? 'you' : member.username} ${member.threads}`)
  return <Text tone="dim">Most active: {named.join(' · ')}</Text>
}

/** Who the thread is shared with, in one short line. */
function sharesLine(shares: Shares): string {
  if (shares.upload === 'off') return 'off'
  if (shares.upload === 'logged-out') return 'logged out'
  if (shares.private) {
    const closing = Shares.orgs(shares, 'closing')
    const pending = [
      ...(closing.length > 0 ? [`closed to ${closing.join(', ')}`] : []),
      ...(shares.revoking_links ? ['its links are revoked'] : []),
    ]
    return pending.length === 0 ? 'private' : `private once ${pending.join(' and ')}`
  }
  const opening = Shares.orgs(shares, 'opening').map(org => `-> ${org}`)
  const closed = Shares.orgs(shares, 'closing', 'closed')
  const parts = [
    ...Shares.orgs(shares, 'open'),
    ...opening,
    ...(closed.length > 0 ? [`closed to ${closed.join(', ')}`] : []),
  ]
  return parts.length === 0 ? 'not shared' : parts.join(', ')
}

/** What the last action pressed in the pane did, as one line. */
function NoticeText({ notice }: { notice: Notice }): ModElement {
  switch (notice.action) {
    case 'private': {
      const { outcome } = notice
      if (!outcome.isOk) return <Text tone="dim">Could not make it private: {outcome.reason}</Text>
      return outcome.value === null ? (
        <Text tone="dim">This thread is not captured.</Text>
      ) : (
        <Text tone="dim">This thread is {sharesLine(outcome.value)}.</Text>
      )
    }
    case 'link': {
      const { outcome } = notice
      if (!outcome.isOk) return <Text tone="dim">No share link: {outcome.reason}</Text>
      return (
        <Text tone="dim">
          {outcome.value.isCopied ? 'Copied' : 'Share link:'} {outcome.value.link}
        </Text>
      )
    }
  }
}

/** Why the pane has no team to show, or nothing when it has one. */
function NoTeam({ team, isOctavoOff }: { team: Team; isOctavoOff: boolean }) {
  if (team === null) return <Text tone="dim">Looking up your team...</Text>
  if (!team.isOk) {
    return isOctavoOff ? (
      <Text tone="dim">Octavo shows your team here. `atuin setup --only --hub` turns it on.</Text>
    ) : (
      <Text tone="dim">Your team shows here once the Octavo MCP server answers: {team.reason}</Text>
    )
  }
  if (team.value.orgs.length === 0) {
    return <Text tone="dim">Join an organization on the hub to see your team here.</Text>
  }
  return null
}

function Stats({ activity, username }: { activity: Activity; username: string | null }) {
  const { totals } = activity
  return (
    <Column>
      <Text>
        {compact(totals.threads)} threads · {compact(totals.tool_calls)} tool calls ·{' '}
        <Text tone="success">+{compact(totals.lines_added)}</Text>{' '}
        <Text tone="error">−{compact(totals.lines_removed)}</Text> lines
      </Text>
      <Text>
        <Text color={ATUIN_GREEN}>{sparkline(activity.days.map(day => day.threads))}</Text>
        <Text tone="dim"> threads a day</Text>
      </Text>
      <MostActive activity={activity} username={username} />
    </Column>
  )
}

// Digits run across the whole pane, so no two orgs' threads share one; past 9 a row has none.
const LAST_KEY = 9
const hotkeyOf = (key: number) => (key <= LAST_KEY ? { hotkey: String(key) } : {})

const rowsOf = ({ threads }: OrgTeam): readonly TeamThread[] =>
  threads.isOk ? threads.value.slice(0, Team.WORKING_ROWS) : []

function Working({
  threads,
  rows,
  firstKey,
  nowMs,
  onView,
}: {
  threads: Outcome<readonly TeamThread[]>
  rows: readonly TeamThread[]
  firstKey: number
  nowMs: number
  onView: (thread: TeamThread) => void
}) {
  return (
    <Column>
      <Text bold>Working now</Text>
      {!threads.isOk && <Text tone="warning">Could not read its threads: {threads.reason}</Text>}
      {threads.isOk && rows.length === 0 && <Text tone="dim">Nobody else in the last day.</Text>}
      {rows.map((thread, row) => (
        <Button key={thread.share_id} {...hotkeyOf(firstKey + row)} onPress={() => onView(thread)}>
          <Text tone="dim">{age({ iso: thread.last_active_at, nowMs }).padStart(3)}</Text> {thread.owner}{' '}
          <Text tone="dim">·</Text> {thread.title}
        </Button>
      ))}
    </Column>
  )
}

function Org({
  team,
  firstKey,
  username,
  nowMs,
  onView,
}: {
  team: OrgTeam
  firstKey: number
  username: string | null
  nowMs: number
  onView: (thread: TeamThread) => void
}) {
  return (
    <Column gap={1}>
      <Column>
        <Text>
          <Text bold>{team.org}</Text>
          <Text tone="dim"> · last 7 days</Text>
        </Text>
        {team.activity.isOk ? (
          <Stats activity={team.activity.value} username={username} />
        ) : (
          <Text tone="warning">Could not count its activity: {team.activity.reason}</Text>
        )}
      </Column>
      <Working threads={team.threads} rows={rowsOf(team)} firstKey={firstKey} nowMs={nowMs} onView={onView} />
    </Column>
  )
}

/** The `/atuin` pane: the thread's sharing, then what each of the user's organizations does. */
export function SharesView({
  report,
  team,
  pane,
  nowMs,
  onPrivate,
  onCopyLink,
  onView,
}: {
  report: Report
  team: Team
  pane: SharesPane
  nowMs: number
  onPrivate: () => Promise<void>
  onCopyLink: (press: Press) => Promise<void>
  onView: (viewed: { org: string; thread: TeamThread }) => void
}) {
  const shares = Report.shares(report)
  // A thread not captured yet has nothing on Atuin Hub to make private or link to.
  const isCaptured = report?.isOk === true && report.value.thread !== null
  const sharing = shares === null ? [] : Shares.sharing(shares)
  const orgs = team?.isOk === true ? team.value.orgs : []
  const username = team?.isOk === true ? team.value.username : null

  return (
    <Column gap={1}>
      <Column>
        <Text>
          <Text bold>This thread </Text>
          {report === null ? <Text tone="dim">looking...</Text> : <StatusView report={report} />}
        </Text>
        {shares?.upload === 'on' && isCaptured && (
          <Row gap={2}>
            {sharing.length > 0 && (
              <Button key="private" hotkey="p" onPress={onPrivate}>
                {pane.isConfirmingPrivate ? `Close to ${sharing.join(', ')} for good` : 'Make private'}
              </Button>
            )}
            <Button key="link" hotkey="l" onPress={onCopyLink}>
              Copy share link
            </Button>
          </Row>
        )}
        {pane.notice !== null && <NoticeText notice={pane.notice} />}
      </Column>
      <NoTeam team={team} isOctavoOff={shares?.upload === 'off'} />
      {orgs.map((org, index) => {
        const firstKey = 1 + orgs.slice(0, index).reduce((rows, one) => rows + rowsOf(one).length, 0)
        return (
          <Org
            key={org.org}
            team={org}
            firstKey={firstKey}
            username={username}
            nowMs={nowMs}
            onView={thread => onView({ org: org.org, thread })}
          />
        )
      })}
    </Column>
  )
}
