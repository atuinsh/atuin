import { Status, Text } from '@atuin/modkit'

import { Report } from './status'

/** Atuin's green and the ink its logo draws on it, which reads there in light and dark themes. */
export const ATUIN_GREEN = '#26bd65'
const ATUIN_INK = '#263238'

/** The share chip, after the mode labels at the right of the prompt footer. */
export function Chip({ report }: { report: Report }) {
  const words = Report.words(report)
  if (words === undefined) return null
  return (
    <Status>
      <Text color={ATUIN_INK} background={ATUIN_GREEN} bold>
        {' atuin '}
      </Text>
      <Text tone={words.tone}>{` ${words.text}`}</Text>
    </Status>
  )
}
