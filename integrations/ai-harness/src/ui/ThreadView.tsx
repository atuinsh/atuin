import { Button, Column, Markdown, Row, Text, type Press } from '@atuin/modkit'

import { Conversation } from '../conversation'
import { LocalThread } from '../local-thread'
import type { Outcome } from '../outcome'
import { Viewed, type Resume } from '../viewed'

const PULL_LABELS = {
  idle: 'Pull into this session',
  pulling: 'Pulling...',
  pulled: 'Pulled into this session',
  failed: 'Pull failed; try again',
} as const

/** How many other agents a local thread offers to continue in, one per digit key. */
const CONTINUE_IN = 3

/** The thread's text: whose and where, then each message that carries text. */
function ConversationMarkdown({ conversation, view }: { conversation: Conversation; view: Viewed }) {
  const { thread } = view.source
  const branch = thread.git_branch === null ? '' : ` on \`${thread.git_branch}\``
  const text = [
    `**${Viewed.owner(view)}** in \`${thread.cwd}\`${branch}`,
    ...Conversation.texts(conversation).map(message => `**${message.role}**: ${message.text}`),
    ...(Conversation.isPartial(conversation) ? ['*Later messages are left out.*'] : []),
    ...(conversation.last_reply === null ? [] : [`**last reply**: ${conversation.last_reply}`]),
  ].join('\n\n')
  return <Markdown text={text} />
}

/** Keys that resume a thread of `harness`'s: in it, and as a new session of each of `agents`. */
function ResumeKeys({
  harness,
  agents,
  onResume,
}: {
  harness: string
  agents: readonly string[]
  onResume: (agent: string, press: Press) => Promise<void>
}) {
  const others = agents.filter(agent => agent !== harness && LocalThread.isContinuableIn(agent))
  return (
    <Row gap={2}>
      <Button key="resume" hotkey="r" onPress={press => onResume(harness, press)}>
        Resume in {LocalThread.agentName(harness)}
      </Button>
      {others.slice(0, CONTINUE_IN).map((agent, index) => (
        <Button key={`continue:${agent}`} hotkey={String(index + 1)} onPress={press => onResume(agent, press)}>
          Continue in {LocalThread.agentName(agent)}
        </Button>
      ))}
    </Row>
  )
}

/** The command the last resume pressed copied, or why it gave none. */
function ResumeText({ resume }: { resume: Resume }) {
  const agent = LocalThread.agentName(resume.agent)
  if (!resume.outcome.isOk) {
    return (
      <Text tone="warning">
        Could not resume it in {agent}: {resume.outcome.reason}
      </Text>
    )
  }
  const { command, isCopied } = resume.outcome.value
  return (
    <Text tone="dim">
      {isCopied ? `Copied; paste it in a terminal to resume it in ${agent}:` : `Run this to resume it in ${agent}:`}{' '}
      {command}
    </Text>
  )
}

/**
 * A thread, read as markdown, with the key that pulls a summary of it in, and for one of this
 * machine's, the keys that resume it here or in `agents`.
 */
export function ThreadView({
  view,
  conversation,
  isPulling,
  agents,
  onPull,
  onResume,
}: {
  view: Viewed
  conversation: Outcome<Conversation> | null
  isPulling: boolean
  agents: readonly string[]
  onPull: () => Promise<void>
  onResume: (agent: string, press: Press) => Promise<void>
}) {
  // A saved `pulling` with no pull under way is one a restart cut short.
  const pull = view.pull === 'pulling' && !isPulling ? 'idle' : view.pull
  return (
    <Column gap={1}>
      <Button key="pull" hotkey="p" onPress={onPull}>
        {PULL_LABELS[pull]}
      </Button>
      {view.source.kind === 'local' && (
        <ResumeKeys harness={view.source.thread.harness} agents={agents} onResume={onResume} />
      )}
      {view.resume !== null && <ResumeText resume={view.resume} />}
      {conversation === null ? (
        <Text tone="dim">Reading {Viewed.owner(view)}'s thread...</Text>
      ) : conversation.isOk ? (
        <ConversationMarkdown conversation={conversation.value} view={view} />
      ) : (
        <Text>Could not read it: {conversation.reason}</Text>
      )}
    </Column>
  )
}
