import { Button, Column, Markdown, Text } from '@atuin/modkit'

import { Conversation } from '../conversation'
import type { Outcome } from '../outcome'
import type { TeamThread } from '../team-thread'
import type { Viewed } from '../viewed'

const PULL_LABELS = {
  idle: 'Pull into this session',
  pulling: 'Pulling...',
  pulled: 'Pulled into this session',
  failed: 'Pull failed; try again',
} as const

/** The thread's text: who and where, then each message that carries text. */
function ConversationMarkdown({ conversation, thread }: { conversation: Conversation; thread: TeamThread }) {
  const text = [
    `**${thread.owner}** in \`${thread.cwd}\`${thread.git_branch === null ? '' : ` on \`${thread.git_branch}\``}`,
    ...Conversation.texts(conversation).map(message => `**${message.role}**: ${message.text}`),
    ...(Conversation.isPartial(conversation) ? ['*Later messages are left out.*'] : []),
    ...(conversation.last_reply === null ? [] : [`**last reply**: ${conversation.last_reply}`]),
  ].join('\n\n')
  return <Markdown text={text} />
}

/** A teammate's thread, read as markdown, and the key that pulls a summary of it in. */
export function ThreadView({
  view,
  conversation,
  isPulling,
  onPull,
}: {
  view: Viewed
  conversation: Outcome<Conversation> | null
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
      {conversation === null ? (
        <Text tone="dim">Reading {view.thread.owner}'s thread...</Text>
      ) : conversation.isOk ? (
        <ConversationMarkdown conversation={conversation.value} thread={view.thread} />
      ) : (
        <Text>Could not read it: {conversation.reason}</Text>
      )}
    </Column>
  )
}
