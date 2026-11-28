import * as v from 'valibot'

// A thread as `atuin threads list --style json` prints it, which leaves out what a thread lacks,
// so one schema reads the list and the store.
const schema = v.pipe(
  v.object({
    harness: v.string(),
    session_id: v.string(),
    atuin_id: v.string(),
    cwd: v.nullish(v.string(), ''),
    git_branch: v.nullish(v.string(), null),
    started_at: v.nullish(v.string(), null),
    updated_at: v.nullish(v.string(), ''),
    message_count: v.nullish(v.number(), 0),
    title: v.nullish(v.string()),
    preview: v.nullish(v.string()),
  }),
  // An empty title or preview says as little as a missing one.
  v.transform(({ preview, ...thread }) => ({ ...thread, title: thread.title || preview || 'Untitled thread' })),
)

/** A thread an agent on this machine captured. */
export type LocalThread = v.InferOutput<typeof schema>

/** Agents by the names their users know them by, keyed as atuin names them. */
const AGENT_NAMES: Readonly<Record<string, string>> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
  opencode: 'opencode',
  pi: 'pi',
  omp: 'omp',
  copilot: 'Copilot',
  grok: 'Grok',
  kiro: 'Kiro',
  amp: 'Amp',
  hermes: 'Hermes',
  vibe: 'Vibe',
}

/** The agents `atuin threads resume --in` continues a thread in. */
const CONTINUABLE_IN: ReadonlySet<string> = new Set([
  'claude-code',
  'codex',
  'copilot',
  'grok',
  'hermes',
  'kiro',
  'opencode',
  'pi',
  'omp',
  'vibe',
])

export const LocalThread = {
  schema,

  /** Whether `atuin threads resume --in` can continue a thread in `harness`. */
  isContinuableIn: (harness: string): boolean => CONTINUABLE_IN.has(harness),

  /** `harness` as its users name it: `claude-code` is `Claude Code`. One atuin doesn't know keeps its name. */
  agentName: (harness: string): string => AGENT_NAMES[harness] ?? harness,

  /** The agents of `threads`, each with how many it ran, the most used first. */
  agents: (threads: readonly LocalThread[]): { harness: string; threads: number }[] => {
    const counts = new Map<string, number>()
    for (const thread of threads) counts.set(thread.harness, (counts.get(thread.harness) ?? 0) + 1)
    return [...counts]
      .map(([harness, count]) => ({ harness, threads: count }))
      .toSorted((a, b) => b.threads - a.threads)
  },
}
