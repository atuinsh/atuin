import { Button, Column, Row, Text, type Press } from '@atuin/modkit'

import { Activity } from './activity'
import { ATUIN_GREEN } from './Chip'
import type { Outcome } from './outcome'
import { Shares } from './shares'
import type { SharesPane } from './shares-pane'
import { Report } from './status'
import { Team, type OrgTeam } from './team'
import type { TeamThread } from './team-thread'

const BARS = '▁▂▃▄▅▆▇█'

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

/** Why the pane has no team to show, as one line. */
const noTeam = ({ team, isOctavoOff }: { team: Team; isOctavoOff: boolean }): string | undefined => {
  if (team === null) return 'Looking up your team...'
  if (!team.isOk) {
    return isOctavoOff
      ? 'Octavo shows your team here. `atuin setup --only --hub` turns it on.'
      : `Your team shows here once the Octavo MCP server answers: ${team.reason}`
  }
  if (team.value.orgs.length === 0) return 'Join an organization on the hub to see your team here.'
  return undefined
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
      {activity.members.some(member => member.threads > 0) && (
        <Text tone="dim">Most active: {Activity.members(activity, username)}</Text>
      )}
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
  const words = Report.words(report)
  const shares = Report.shares(report)
  const sharing = shares === null ? [] : Shares.sharing(shares)
  const orgs = team?.isOk === true ? team.value.orgs : []
  const username = team?.isOk === true ? team.value.username : null
  const missing = noTeam({ team, isOctavoOff: shares?.upload === 'off' })

  return (
    <Column gap={1}>
      <Column>
        <Text>
          <Text bold>This thread </Text>
          {words === undefined ? (
            <Text tone="dim">looking...</Text>
          ) : (
            <Text tone={words.tone}>{words.text}</Text>
          )}
        </Text>
        {shares?.upload === 'on' && (
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
        {pane.notice !== null && <Text tone="dim">{pane.notice}</Text>}
      </Column>
      {missing !== undefined && <Text tone="dim">{missing}</Text>}
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
