/** How far one of a thread's organizations got, as the daemon's queue has it. */
export type ShareState = 'opening' | 'open' | 'closing' | 'closed'

/** Whether the daemon uploads this machine's threads to Octavo. */
export type Upload = 'on' | 'off' | 'logged-out'

/** Who a captured thread is shared with on Octavo. */
export type ThreadShares = {
  upload: Upload
  orgs: readonly { slug: string; state: ShareState }[]
  /** Each directory the thread had a row uploaded from, with the orgs its config shared with. */
  dirs: readonly { dir: string; orgs: readonly string[] }[]
  /** Whether the user made the thread private, which no config shares again. */
  isPrivate: boolean
}

/** Whether atuin captures coding agents' threads as they happen. */
export type Capture = 'on' | 'off'

/** What `atuin threads status <id> --style json` prints: `thread` and `octavo` null until captured. */
export type ThreadStatus = {
  capture: Capture
  thread: { messages: number; updatedAt: string | null } | null
  octavo: ThreadShares | null
}

/** The thread's status, or why the CLI could not say. */
export type StatusReport =
  | { kind: 'known'; status: ThreadStatus }
  | { kind: 'unavailable'; reason: string }

/** What `atuin workspace orgs <dir> --style json` prints: `config` null when no file decides. */
export type Workspace = { orgs: readonly string[]; config: string | null }
