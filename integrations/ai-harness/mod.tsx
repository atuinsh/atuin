/**
 * What atuin does with a session's thread, and what your team works on.
 *
 * - The footer's chip says whether atuin syncs the thread, and who can open it on Atuin Hub.
 * - `/atuin` toggles a pane of the thread's sharing and each organization's week, from which a
 *   teammate's thread opens in a pane of its own.
 * - A move into a directory whose config shares with fewer organizations asks first.
 * - A card under a failed command lists the threads that hit the same error.
 */
import { Pane, mod, type Press, type ShellResult } from '@atuin/modkit'
import { useHarness, useQuery, useState } from '@atuin/modkit/ui/hooks'

import { Atuin } from './src/atuin'
import { LocalThread } from './src/local-thread'
import { Moves } from './src/moves'
import { Octavo } from './src/octavo'
import { Outcome } from './src/outcome'
import { Pull } from './src/pull'
import { SeenCard, SeenLookup } from './src/seen'
import type { Notice } from './src/shares-pane'
import { Report } from './src/status'
import {
  conversation,
  earlierThreads,
  paneOpen,
  recentThreads,
  report as threadReport,
  searchedErrors,
  seenCards,
  sharesPane,
  team as orgTeam,
  viewedThread,
} from './src/store'
import { Chip } from './src/ui/Chip'
import { SeenView } from './src/ui/SeenView'
import { SharesView } from './src/ui/SharesView'
import { ThreadView } from './src/ui/ThreadView'
import { Viewed, type Source } from './src/viewed'

// Pulls under way in this process, by session and thread: one doesn't outlive the process.
const pulling = new Set<string>()

/** The most a thread pane's title holds. */
const TITLE_CHARS = 60

interface Settings {
  /**
   * The atuin executable the mod runs, such as a dev build's path
   * @title atuin binary
   */
  atuinPath?: string
  /**
   * The name /mcp lists the Octavo MCP server under
   * @title Octavo MCP server
   */
  mcpServer?: string
  /**
   * The model that summarises a thread you pull in, by alias or id
   * @title Summary model
   */
  summaryModel?: string
}

export default mod(({ atuinPath = 'atuin', mcpServer = 'atuin', summaryModel = 'haiku' }: Settings) => {
  const harness = useHarness()
  const atuin = new Atuin(harness, { atuinPath })
  const octavo = new Octavo(harness, { mcpServer })
  const report = useQuery(threadReport, atuin.settings)
  const moves = new Moves({ harness, atuin, shares: Report.shares(report.value) })
  const [isOpen, setIsOpen] = useState(paneOpen)
  const [pane, setPane] = useState(sharesPane)
  const [viewed, setViewed] = useState(viewedThread)
  const [seen, setSeen] = useState(seenCards)
  const [searched, setSearched] = useState(searchedErrors)
  const read = useQuery(
    conversation,
    viewed === null ? null : { octavo: octavo.settings, atuin: atuin.settings, source: viewed.source },
  )
  const pullKey = `${harness.session.id}:${viewed === null ? '' : Viewed.key(viewed)}`
  const team = useQuery(orgTeam, isOpen ? octavo.settings : null)
  // The thread pane offers to continue a thread in the agents of this machine's recent threads.
  const recent = useQuery(recentThreads, isOpen || viewed !== null ? atuin.settings : null)
  const earlier = useQuery(earlierThreads, isOpen ? atuin.settings : null)
  const agents = recent.value?.isOk === true ? LocalThread.agents(recent.value.value).map(agent => agent.harness) : []

  const setNotice = (notice: Notice) => {
    setPane(view => ({ ...view, notice, isConfirmingPrivate: false }))
  }

  /** Asks for confirmation on the first press, and closes the thread to every org on the second. */
  const makePrivate = async () => {
    if (!pane.isConfirmingPrivate) {
      setPane(view => ({ ...view, isConfirmingPrivate: true, notice: null }))
      return
    }
    const outcome = await atuin.unshare()
    if (outcome.isOk) void report.refresh()
    setNotice({ action: 'private', outcome: outcome.isOk ? Outcome.ok(outcome.value.octavo) : outcome })
  }

  /** Copies the thread's share link to the clipboard of the surface pressed on. */
  const copyLink = async ({ surface }: Press) => {
    const link = await atuin.share()
    if (!link.isOk) {
      setNotice({ action: 'link', outcome: link })
      return
    }
    const copied = await harness.copy(link.value, { surface })
    setNotice({ action: 'link', outcome: Outcome.ok({ link: link.value, isCopied: copied.isCopied }) })
  }

  const view = (source: Source) => {
    setViewed(Viewed.of(source))
  }

  /** Updates the thread in view with `change`, if it's still the thread `key` names. */
  const updateViewed = (key: string, change: (now: Viewed) => Viewed) => {
    setViewed(now => (now !== null && Viewed.key(now) === key ? change(now) : now))
  }

  /** Summarises the thread in view into the session, once at a time. */
  const pull = async () => {
    if (viewed === null || pulling.has(pullKey)) return
    const key = Viewed.key(viewed)
    const { source } = viewed
    const [readThread, readMore] =
      source.kind === 'team'
        ? [
            () => octavo.conversation(source.thread.share_id),
            `Read more of it with the ${octavo.settings.mcpServer} MCP server's get_thread tool, passing share_id ${source.thread.share_id}.`,
          ]
        : [
            () => atuin.conversation(source.thread.atuin_id),
            `Read more of it with \`atuin threads show ${source.thread.atuin_id}\`.`,
          ]
    pulling.add(pullKey)
    updateViewed(key, now => ({ ...now, pull: 'pulling' }))
    const pulled = await new Pull({ harness, summaryModel })
      .of({ source, read: readThread, readMore })
      .finally(() => pulling.delete(pullKey))
    updateViewed(key, now => ({ ...now, pull: pulled.isOk ? 'pulled' : 'failed' }))
    if (!pulled.isOk) harness.notify(`Could not pull it in: ${pulled.reason}`)
  }

  /** Copies the command that resumes this machine's thread in view in `agent`, where pressed. */
  const resume = async (agent: string, { surface }: Press) => {
    if (viewed?.source.kind !== 'local') return
    const key = Viewed.key(viewed)
    const { thread } = viewed.source
    const command = await atuin.resumeCommand({
      id: thread.atuin_id,
      ...(agent !== thread.harness && { agent }),
    })
    const outcome = command.isOk
      ? Outcome.ok({ command: command.value, isCopied: (await harness.copy(command.value, { surface })).isCopied })
      : command
    updateViewed(key, now => ({ ...now, resume: { agent, outcome } }))
  }

  /** Looks a failed command's error up once, to card the threads that hit it too. */
  const lookUp = (result: ShellResult) => {
    if (!result.failed) return
    const query = SeenCard.query(result.output)
    if (query === undefined || searched.includes(query)) return
    setSearched(all => [...all, query])
    void new SeenLookup({ atuin, octavo }).hits(query).then(hits => {
      if (hits.length > 0) setSeen(all => ({ ...all, [result.id]: { query, hits, isAdded: false } }))
    })
  }

  const addSeen = async (id: string) => {
    const card = seen[id]
    if (card === undefined || card.isAdded) return
    if ((await harness.inject(SeenCard.note(card, octavo.settings.mcpServer))) === 'dropped') {
      harness.notify('The session refused these threads; try again.')
      return
    }
    setSeen(all => ({ ...all, [id]: { ...card, isAdded: true } }))
  }

  const toggle = () => {
    if (isOpen) {
      setIsOpen(false)
      return 'Pane closed.'
    }
    void report.refresh()
    setIsOpen(true)
    return 'Pane opened.'
  }

  return {
    status: () => <Chip report={report.value} />,
    commands: {
      atuin: {
        description: 'Toggle the atuin pane: this thread, and what your team works on',
        run: toggle,
      },
    },
    panes: [
      () =>
        isOpen && (
          <Pane
            id="atuin-octavo"
            title="atuin"
            onClose={() => {
              setIsOpen(false)
            }}
          >
            <SharesView
              report={report.value}
              team={team.value}
              recent={recent.value}
              earlier={earlier.value}
              sessionId={harness.session.id}
              pane={pane}
              nowMs={harness.now()}
              onPrivate={makePrivate}
              onCopyLink={copyLink}
              onView={view}
            />
          </Pane>
        ),
      () =>
        viewed !== null && (
          <Pane
            id="atuin-thread"
            title={`${Viewed.owner(viewed)}: ${viewed.source.thread.title}`.slice(0, TITLE_CHARS)}
            onClose={() => {
              setViewed(null)
            }}
          >
            <ThreadView
              view={viewed}
              conversation={read.value}
              isPulling={pulling.has(pullKey)}
              agents={agents}
              onPull={pull}
              onResume={resume}
            />
          </Pane>
        ),
    ],
    beforeShell: async call => moves.ofShell(call),
    beforeTool: {
      EnterWorktree: async call => moves.ofWorktree(call),
    },
    afterTool: {
      EnterWorktree: () => {
        void report.refresh()
      },
    },
    afterShell: lookUp,
    toolCards: {
      Bash: row => {
        const card = seen[row.id]
        return card === undefined ? null : <SeenView card={card} onAdd={() => addSeen(row.id)} />
      },
    },
  }
})
