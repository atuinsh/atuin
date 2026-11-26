import { mod } from '@atuin/modkit'
import { useHarness, useQuery } from '@atuin/modkit/ui/hooks'

import { Chip } from './src/Chip'
import { configOf } from './src/config'
import { report as threadReport } from './src/held'
import { bashTarget, narrowing, worktreeTarget } from './src/narrowing'
import { octavoOf } from './src/status'

interface Settings {
  /**
   * The atuin executable the mod runs, such as a dev build's path
   * @title atuin binary
   */
  atuinPath?: string
}

export default mod(({ atuinPath = 'atuin' }: Settings) => {
  const harness = useHarness()
  const config = configOf({ atuinPath })
  const report = useQuery(threadReport, config)

  const shares = octavoOf(report.value)

  return {
    status: () => <Chip report={report.value} />,
    beforeShell: async call => narrowing(harness, { config, shares, dir: await bashTarget(harness, call) }),
    beforeTool: {
      EnterWorktree: call => narrowing(harness, { config, shares, dir: worktreeTarget(call) }),
    },
    afterTool: {
      EnterWorktree: () => {
        void report.refresh()
      },
    },
  }
})
