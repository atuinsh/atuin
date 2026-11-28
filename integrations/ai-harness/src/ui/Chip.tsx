import { Img, Status, Text } from '@atuin/modkit'

import type { Report } from '../status'
import { StatusView } from './StatusView'

/** The atuin chip, after the mode labels at the right of the prompt footer. */
export function Chip({ report }: { report: Report }) {
  if (report === null) return null
  return (
    <Status>
      <Img image="art/turtle.png" fallback="🐢" />
      <Text> </Text>
      <StatusView report={report} />
    </Status>
  )
}
