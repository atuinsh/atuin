import { Button, Column, Text } from '@atuin/modkit'

import type { SeenCard } from './seen'

/** How much of a hit's snippet the card shows. */
const SNIPPET_CHARS = 160

/** The card under a failed command: the threads that hit the same error, and a key to add them. */
export function SeenView({ card, onAdd }: { card: SeenCard; onAdd: () => Promise<void> }) {
  return (
    <Column border="round" borderTone="dim" paddingX={1}>
      <Text tone="dim">Seen before in Octavo</Text>
      {card.hits.map(hit => (
        <Text key={hit.thread_id}>
          {hit.owner ?? 'you'}: {hit.title} <Text tone="dim">{hit.snippet.slice(0, SNIPPET_CHARS)}</Text>
        </Text>
      ))}
      <Button key="add" onPress={onAdd}>
        {card.isAdded ? 'Added to context' : 'Add to context'}
      </Button>
    </Column>
  )
}
