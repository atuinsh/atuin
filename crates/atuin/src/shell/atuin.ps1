# Atuin PowerShell module
#
# Usage: atuin init powershell | Out-String | Invoke-Expression

if (Get-Module Atuin -ErrorAction Ignore) {
    Write-Warning "The Atuin module is already loaded."
    return
}

if (!(Get-Command atuin -ErrorAction Ignore)) {
    Write-Error "The 'atuin' executable needs to be available in the PATH."
    return
}

if (!(Get-Module PSReadLine -ErrorAction Ignore)) {
    Write-Error "Atuin requires the PSReadLine module to be installed."
    return
}

New-Module -Name Atuin -ScriptBlock {
    $env:ATUIN_SESSION = atuin uuid

    $script:atuinHistoryId = $null
    $script:previousPSConsoleHostReadLine = $Function:PSConsoleHostReadLine

    # The ReadLine overloads changed with breaking changes over time, make sure the one we expect is available.
    $script:hasExpectedReadLineOverload = ([Microsoft.PowerShell.PSConsoleReadLine]::ReadLine).OverloadDefinitions.Contains("static string ReadLine(runspace runspace, System.Management.Automation.EngineIntrinsics engineIntrinsics, System.Threading.CancellationToken cancellationToken, System.Nullable[bool] lastRunStatus)")

    function PSConsoleHostReadLine {
        # This needs to be done as the first thing because any script run will flush $?.
        $lastRunStatus = $?

        # Exit statuses are maintained separately for native and PowerShell commands, this needs to be taken into account.
        $exitCode = if ($lastRunStatus) { 0 } elseif ($global:LASTEXITCODE) { $global:LASTEXITCODE } else { 1 }

        if ($script:atuinHistoryId) {
            # The duration is not recorded in old PowerShell versions, let Atuin handle it.
            $duration = (Get-History -Count 1).Duration.Ticks * 100
            $durationArg = if ($duration) { "--duration=$duration" } else { "" }

            atuin history end --exit=$exitCode $durationArg -- $script:atuinHistoryId | Out-Null

            $global:LASTEXITCODE = $exitCode
            $script:atuinHistoryId = $null
        }

        # PSConsoleHostReadLine implementation from PSReadLine, adjusted to support old versions.
        Microsoft.PowerShell.Core\Set-StrictMode -Off

        $line = if ($script:hasExpectedReadLineOverload) {
            # When the overload we expect is available, we can pass $lastRunStatus to it.
            [Microsoft.PowerShell.PSConsoleReadLine]::ReadLine($Host.Runspace, $ExecutionContext, [System.Threading.CancellationToken]::None, $lastRunStatus)
        } else {
            # Either PSReadLine is older than v2.2.0-beta3, or maybe newer than we expect, so use the function from PSReadLine as-is.
            & $script:previousPSConsoleHostReadLine
        }

        $script:atuinHistoryId = atuin history start -- $line

        return $line
    }

    function RunSearch {
        param([string]$ExtraArgs = "")

        $previousOutputEncoding = [System.Console]::OutputEncoding
        $resultFile = New-TemporaryFile

        try {
            [System.Console]::OutputEncoding = [System.Text.Encoding]::UTF8

            $line = $null
            [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$null)

            # Atuin is started through Start-Process to avoid interfering with the current shell.
            $env:ATUIN_SHELL_POWERSHELL = "true"
            $argString = "search -i --result-file ""$resultFile"" $ExtraArgs -- $line"
            Start-Process -Wait -NoNewWindow -FilePath atuin -ArgumentList $argString
            $suggestion = (Get-Content -Raw $resultFile -Encoding UTF8 | Out-String).Trim()

            # PSReadLine maintains its own cursor position, which will no longer be valid if Atuin scrolls the display in inline mode.
            # Fortunately, InvokePrompt can receive a new Y position and reset the internal state.
            [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt($null, $Host.UI.RawUI.CursorPosition.Y + [int]$env:ATUIN_POWERSHELL_PROMPT_OFFSET)

            if ($suggestion -eq "") {
                # The previous input was already rendered by InvokePrompt
                return
            }

            $acceptPrefix = "__atuin_accept__:"

            if ( $suggestion.StartsWith($acceptPrefix)) {
                [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
                [Microsoft.PowerShell.PSConsoleReadLine]::Insert($suggestion.Substring($acceptPrefix.Length))
                [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
            } else {
                [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
                [Microsoft.PowerShell.PSConsoleReadLine]::Insert($suggestion)
            }
        }
        finally {
            [System.Console]::OutputEncoding = $previousOutputEncoding
            $env:ATUIN_SHELL_POWERSHELL = $null
            Remove-Item $resultFile
        }
    }

    function Enable-AtuinSearchKeys {
        param([bool]$CtrlR = $true, [bool]$UpArrow = $true)

        if ($CtrlR) {
            Set-PSReadLineKeyHandler -Chord "Ctrl+r" -BriefDescription "Runs Atuin search" -ScriptBlock {
                RunSearch
            }
        }

        if ($UpArrow) {
            Set-PSReadLineKeyHandler -Chord "UpArrow" -BriefDescription "Runs Atuin search" -ScriptBlock {
                $line = $null
                [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$line, [ref]$null)

                if (!$line.Contains("`n")) {
                    RunSearch -ExtraArgs "--shell-up-key-binding"
                } else {
                    [Microsoft.PowerShell.PSConsoleReadLine]::PreviousLine()
                }
            }
        }
    }

    $ExecutionContext.SessionState.Module.OnRemove += {
        $env:ATUIN_SESSION = $null
        $Function:PSConsoleHostReadLine = $script:previousPSConsoleHostReadLine
    }

    Export-ModuleMember -Function @("Enable-AtuinSearchKeys", "PSConsoleHostReadLine")
} | Import-Module -Global
