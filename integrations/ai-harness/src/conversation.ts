import * as v from 'valibot'

/** How much of a conversation the summary reads; past it the middle is left out. */
const TRANSCRIPT_CHARS = 40_000

// As `get_thread` answers at its `conversation` detail.
const schema = v.object({
  messages: v.array(
    v.object({ role: v.nullish(v.string(), 'other'), text: v.nullish(v.string(), '') }),
  ),
  next_cursor: v.nullish(v.string(), null),
  last_reply: v.nullish(v.string(), null),
})

/** The first page of a shared thread's conversation, whether more follows it, and its last reply. */
export type Conversation = v.InferOutput<typeof schema>

/** A conversation's messages that carry text, without the bare tool-call markers. */
const texts = ({ messages }: Conversation): { role: string; text: string }[] =>
  messages.flatMap(({ role, text }) => {
    const said = text.trim()
    return said === '' || /^(\[tool_call [^\]]+\]\s*)+$/.test(said) ? [] : [{ role, text: said }]
  })

const isPartial = (conversation: Conversation): boolean => (conversation.next_cursor ?? '') !== ''

export const Conversation = {
  schema,
  texts,

  /** Whether more of the conversation follows the page read. */
  isPartial,

  /** The conversation as one text for the summary, its middle cut when it is too long. */
  transcript: (conversation: Conversation): string => {
    const whole = [
      ...texts(conversation),
      ...(isPartial(conversation) ? [{ role: 'note', text: 'later messages are left out here' }] : []),
      ...(conversation.last_reply === null ? [] : [{ role: 'last reply', text: conversation.last_reply }]),
    ]
      .map(message => `${message.role}: ${message.text}`)
      .join('\n\n')
    if (whole.length <= TRANSCRIPT_CHARS) return whole
    const half = TRANSCRIPT_CHARS / 2
    return `${whole.slice(0, half)}\n\n[... middle left out ...]\n\n${whole.slice(-half)}`
  },
}
