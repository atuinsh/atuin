import { mod } from '@atuin/modkit'
import { useHarness, useQuery } from '@atuin/modkit/ui/hooks'

import { Atuin } from './src/atuin'
import { Chip } from './src/Chip'
import { Moves } from './src/moves'
import { Report } from './src/status'
import { report as threadReport } from './src/store'

interface Settings {
  /**
   * The atuin executable the mod runs, such as a dev build's path
   * @title atuin binary
   */
  atuinPath?: string
}

export default mod(({ atuinPath = 'atuin' }: Settings) => {
  const harness = useHarness()
  const atuin = new Atuin(harness, { atuinPath })
  const report = useQuery(threadReport, atuin.settings)
  const moves = new Moves({ harness, atuin, shares: Report.shares(report.value) })

  return {
    status: () => <Chip report={report.value} />,
    beforeShell: async call => moves.ofShell(call),
    beforeTool: {
      EnterWorktree: async call => moves.ofWorktree(call),
    },
    afterTool: {
      EnterWorktree: () => {
        void report.refresh()
      },
    },
  }
})
