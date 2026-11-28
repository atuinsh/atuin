import { Button, Column, Row, Text, type ModElement, type Press } from '@atuin/modkit'

import type { Activity } from '../activity'
import { LocalThread } from '../local-thread'
import type { Outcome } from '../outcome'
import { Shares } from '../shares'
import type { Notice, SharesPane } from '../shares-pane'
import { Report } from '../status'
import { EARLIER_THREADS } from '../store'
import { Team, type OrgTeam } from '../team'
import type { TeamThread } from '../team-thread'
import type { Source } from '../viewed'
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
  if (shares.upload === 'turned-off') return 'private'
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
/** How long after its last activity a thread of this machine's counts as worked on now. */
const WORKING_WITHIN_MS = 24 * 60 * 60 * 1000

/** How many days the stats of this machine's threads count, as an org's do. */
const LOCAL_DAYS = 7
const DAY_MS = 24 * 60 * 60 * 1000

/** The local day `ms` falls on, as `2026-11-27`. */
const dayOf = (ms: number): string => new Date(ms).toLocaleDateString('en-CA')

/**
 * This machine's threads active this week: how many, the messages in them, including those from
 * before the week, threads started a day, and the agents.
 */
function LocalStats({ threads, nowMs }: { threads: readonly LocalThread[]; nowMs: number }) {
  const days = Array.from({ length: LOCAL_DAYS }, (_, index) => dayOf(nowMs - (LOCAL_DAYS - 1 - index) * DAY_MS))
  const started = threads.flatMap(thread => (thread.started_at === null ? [] : [dayOf(Date.parse(thread.started_at))]))
  const messages = threads.reduce((sum, thread) => sum + thread.message_count, 0)
  const agents = LocalThread.agents(threads)
    .slice(0, ACTIVE_MEMBERS)
    .map(agent => `${LocalThread.agentName(agent.harness)} ${agent.threads}`)
  return (
    <Column>
      <Text>
        {compact(threads.length)} threads · {compact(messages)} messages in them
      </Text>
      <Text>
        <Text color={ATUIN_GREEN}>{sparkline(days.map(day => started.filter(one => one === day).length))}</Text>
        <Text tone="dim"> threads started a day</Text>
      </Text>
      {agents.length > 0 && <Text tone="dim">Most used: {agents.join(' · ')}</Text>}
    </Column>
  )
}

/** A thread of this machine's, its key opening it in the thread pane. */
function LocalRow({
  thread,
  hotkey,
  nowMs,
  onView,
}: {
  thread: LocalThread
  hotkey: number
  nowMs: number
  onView: (source: Source) => void
}) {
  return (
    <Button {...hotkeyOf(hotkey)} onPress={() => onView({ kind: 'local', thread })}>
      <Text tone="dim">{age({ iso: thread.updated_at, nowMs }).padStart(3)}</Text>{' '}
      {LocalThread.agentName(thread.harness)} <Text tone="dim">·</Text> {thread.title}
    </Button>
  )
}

/** Where there's no team to show: what the user's own agents did this week, and work on now. */
function LocalAgents({
  threads,
  rows,
  firstKey,
  nowMs,
  onView,
}: {
  threads: readonly LocalThread[]
  rows: readonly LocalThread[]
  firstKey: number
  nowMs: number
  onView: (source: Source) => void
}) {
  return (
    <Column gap={1}>
      <Column>
        <Text>
          <Text bold>Your agents</Text>
          <Text tone="dim"> · last {LOCAL_DAYS} days</Text>
        </Text>
        <LocalStats threads={threads} nowMs={nowMs} />
      </Column>
      <Column>
        <Text bold>Working now</Text>
        {rows.length === 0 && <Text tone="dim">No other thread in the last day.</Text>}
        {rows.map((thread, row) => (
          <LocalRow key={thread.atuin_id} thread={thread} hotkey={firstKey + row} nowMs={nowMs} onView={onView} />
        ))}
      </Column>
    </Column>
  )
}

/** This project's earlier threads, from any agent, to read, pull in, or resume. */
function EarlierThreads({
  rows,
  firstKey,
  nowMs,
  onView,
}: {
  rows: readonly LocalThread[]
  firstKey: number
  nowMs: number
  onView: (source: Source) => void
}) {
  if (rows.length === 0) return null
  return (
    <Column>
      <Text bold>Earlier threads here</Text>
      {rows.map((thread, row) => (
        <LocalRow key={thread.atuin_id} thread={thread} hotkey={firstKey + row} nowMs={nowMs} onView={onView} />
      ))}
    </Column>
  )
}

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

/**
 * The `/atuin` pane: the thread's sharing, then what each of the user's organizations does, or
 * with none, what their own agents do, then this project's earlier threads.
 */
export function SharesView({
  report,
  team,
  recent,
  earlier,
  sessionId,
  pane,
  nowMs,
  onPrivate,
  onCopyLink,
  onView,
}: {
  report: Report
  team: Team
  recent: Outcome<readonly LocalThread[]> | null
  earlier: Outcome<readonly LocalThread[]> | null
  sessionId: string
  pane: SharesPane
  nowMs: number
  onPrivate: () => Promise<void>
  onCopyLink: (press: Press) => Promise<void>
  onView: (source: Source) => void
}) {
  const shares = Report.shares(report)
  // A thread not captured yet has nothing on Atuin Hub to make private or link to.
  const isCaptured = report?.isOk === true && report.value.thread !== null
  const sharing = shares === null ? [] : Shares.sharing(shares)
  const orgs = team?.isOk === true ? team.value.orgs : []
  const username = team?.isOk === true ? team.value.username : null
  // Asked for only once the team is known, so a team that comes later doesn't move the rows.
  const isTeamless = team !== null && orgs.length === 0
  const others = (threads: typeof recent) =>
    threads?.isOk === true ? threads.value.filter(thread => thread.session_id !== sessionId) : []
  const working = isTeamless
    ? others(recent)
        .filter(thread => nowMs - Date.parse(thread.updated_at) < WORKING_WITHIN_MS)
        .slice(0, Team.WORKING_ROWS)
    : []
  const listed = new Set(working.map(thread => thread.atuin_id))
  const earlierRows = others(earlier)
    .filter(thread => !listed.has(thread.atuin_id))
    .slice(0, EARLIER_THREADS)
  const rowsAbove = isTeamless ? working.length : orgs.reduce((rows, one) => rows + rowsOf(one).length, 0)

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
      {orgs.map((org, index) => {
        const firstKey = 1 + orgs.slice(0, index).reduce((rows, one) => rows + rowsOf(one).length, 0)
        return (
          <Org
            key={org.org}
            team={org}
            firstKey={firstKey}
            username={username}
            nowMs={nowMs}
            onView={thread => onView({ kind: 'team', org: org.org, thread })}
          />
        )
      })}
      {isTeamless && recent !== null && !recent.isOk && (
        <Text tone="warning">Could not list this machine's threads: {recent.reason}</Text>
      )}
      {isTeamless && recent?.isOk === true && (
        <LocalAgents threads={others(recent)} rows={working} firstKey={1} nowMs={nowMs} onView={onView} />
      )}
      <EarlierThreads rows={earlierRows} firstKey={1 + rowsAbove} nowMs={nowMs} onView={onView} />
      <NoTeam team={team} isOctavoOff={shares?.upload === 'off' || shares?.upload === 'turned-off'} />
    </Column>
  )
}
