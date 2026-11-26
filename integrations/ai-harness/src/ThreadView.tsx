import { Button, Column, Markdown, Text } from '@atuin/modkit'

import type { Viewed } from './viewed'

const PULL_LABELS = {
  idle: 'Pull into this session',
  pulling: 'Pulling...',
  pulled: 'Pulled into this session',
  failed: 'Pull failed; try again',
} as const

/** A teammate's thread, read as markdown, and the key that pulls a summary of it in. */
export function ThreadView({
  view,
  markdown,
  isPulling,
  onPull,
}: {
  view: Viewed
  markdown: string | null
  isPulling: boolean
  onPull: () => Promise<void>
}) {
  // A saved `pulling` with no pull under way is one a restart cut short.
  const pull = view.pull === 'pulling' && !isPulling ? 'idle' : view.pull
  return (
    <Column gap={1}>
      <Button key="pull" hotkey="p" onPress={onPull}>
        {PULL_LABELS[pull]}
      </Button>
      {markdown === null ? (
        <Text tone="dim">Reading {view.thread.owner}'s thread...</Text>
      ) : (
        <Markdown text={markdown} />
      )}
    </Column>
  )
}
