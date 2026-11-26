/**
 * What atuin does with a session's thread, and what your team works on.
 *
 * - The footer's chip says whether atuin captures the thread, and who it's shared with on Octavo.
 * - `/atuin` toggles a pane of the thread's sharing and each organization's week, from which a
 *   teammate's thread opens in a pane of its own.
 * - A move into a directory whose config shares with fewer organizations asks first.
 * - A card under a failed command lists the threads that hit the same error.
 */
import { Pane, mod, type Press, type ShellResult } from '@atuin/modkit'
import { useHarness, useQuery, useState } from '@atuin/modkit/ui/hooks'

import { Atuin } from './src/atuin'
import { Chip } from './src/Chip'
import { Moves } from './src/moves'
import { Octavo } from './src/octavo'
import { Pull } from './src/pull'
import { SeenCard, SeenLookup } from './src/seen'
import { SeenView } from './src/SeenView'
import { Shares } from './src/shares'
import { SharesView } from './src/SharesView'
import { Report } from './src/status'
import {
  conversation,
  paneOpen,
  report as threadReport,
  searchedErrors,
  seenCards,
  sharesPane,
  team as orgTeam,
  viewedThread,
} from './src/store'
import type { TeamThread } from './src/team-thread'
import { ThreadView } from './src/ThreadView'
import type { Viewed } from './src/viewed'

// Pulls under way in this process, by session and share: one doesn't outlive the process.
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
  const read = useQuery(conversation, viewed === null ? null : { settings: octavo.settings, thread: viewed.thread })
  const pullKey = `${harness.session.id}:${viewed?.thread.share_id ?? ''}`
  const team = useQuery(orgTeam, isOpen ? octavo.settings : null)

  const setNotice = (notice: string) => {
    setPane(view => ({ ...view, notice, isConfirmingPrivate: false }))
  }

  /** Asks for confirmation on the first press, and closes the thread to every org on the second. */
  const makePrivate = async () => {
    if (!pane.isConfirmingPrivate) {
      setPane(view => ({ ...view, isConfirmingPrivate: true, notice: null }))
      return
    }
    const outcome = await atuin.unshare()
    if (!outcome.isOk) {
      setNotice(`Could not make it private: ${outcome.reason}`)
      return
    }
    void report.refresh()
    const after = outcome.value.octavo
    setNotice(after === null ? 'This thread is not captured.' : `This thread is ${Shares.line(after)} now.`)
  }

  /** Copies the thread's share link to the clipboard of the surface pressed on. */
  const copyLink = async ({ surface }: Press) => {
    const link = await atuin.link()
    if (!link.isOk) {
      setNotice(`No share link: ${link.reason}`)
      return
    }
    const copied = await harness.copy(link.value, { surface })
    setNotice(copied.isCopied ? `Copied ${link.value}` : `Share link: ${link.value}`)
  }

  const view = (shared: { org: string; thread: TeamThread }) => {
    setViewed({ ...shared, pull: 'idle' })
  }

  /** Summarises the thread in view into the session, once at a time. */
  const pull = async () => {
    if (viewed === null || pulling.has(pullKey)) return
    const { thread } = viewed
    const mark = (state: Viewed['pull']) => {
      setViewed(now => (now?.thread.share_id === thread.share_id ? { ...now, pull: state } : now))
    }
    pulling.add(pullKey)
    mark('pulling')
    const pulled = await new Pull({ harness, octavo, summaryModel })
      .of(viewed)
      .finally(() => pulling.delete(pullKey))
    mark(pulled.isOk ? 'pulled' : 'failed')
    if (!pulled.isOk) harness.notify(`Could not pull it in: ${pulled.reason}`)
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
            title={`${viewed.thread.owner}: ${viewed.thread.title}`.slice(0, TITLE_CHARS)}
            onClose={() => {
              setViewed(null)
            }}
          >
            <ThreadView view={viewed} markdown={read.value} isPulling={pulling.has(pullKey)} onPull={pull} />
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
