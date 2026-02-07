# Atuin PowerShell module
#
# This should support PowerShell 5.1 (which is shipped with Windows) and later versions, on Windows and Linux.
#
# Usage: atuin init powershell | Out-String | Invoke-Expression
#
# Settings:
# - $env:ATUIN_POWERSHELL_PROMPT_OFFSET - Number of lines to offset the prompt position after exiting search.
#   This is useful when using a multi-line prompt: e.g. set this to -1 when using a 2-line prompt.
#   It is initialized from the current prompt line count if not set when the first Atuin search is performed.

if (Get-Module Atuin -ErrorAction Ignore) {
    if ($PSVersionTable.PSVersion.Major -ge 7) {
        Write-Warning "The Atuin module is already loaded, replacing it."
        Remove-Module Atuin
    } else {
        Write-Warning "The Atuin module is already loaded, skipping."
        return
    }
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
    if (-not $env:ATUIN_SESSION -or $env:ATUIN_PID -ne $PID) {
        $env:ATUIN_SESSION = atuin uuid
        $env:ATUIN_PID = $PID
    }

    $script:atuinHistoryId = $null
    $script:previousPSConsoleHostReadLine = $Function:PSConsoleHostReadLine

    # The ReadLine overloads changed with breaking changes over time, make sure the one we expect is available.
    $script:hasExpectedReadLineOverload = ([Microsoft.PowerShell.PSConsoleReadLine]::ReadLine).OverloadDefinitions.Contains("static string ReadLine(runspace runspace, System.Management.Automation.EngineIntrinsics engineIntrinsics, System.Threading.CancellationToken cancellationToken, System.Nullable[bool] lastRunStatus)")

    function Get-CommandLine {
        $commandLine = ""
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$commandLine, [ref]$null)
        return $commandLine
    }

    function Set-CommandLine {
        param([string]$Text)

        $commandLine = Get-CommandLine
        [Microsoft.PowerShell.PSConsoleReadLine]::Replace(0, $commandLine.Length, $Text)
    }

    # This function name is called by PSReadLine to read the next command line to execute.
    # We replace it with a custom implementation which adds Atuin support.
    function PSConsoleHostReadLine {
        ## 1. Collect the exit code of the previous command.

        # This needs to be done as the first thing because any script run will flush $?.
        $lastRunStatus = $?

        # Exit statuses are maintained separately for native and PowerShell commands, this needs to be taken into account.
        $lastNativeExitCode = $global:LASTEXITCODE
        $exitCode = if ($lastRunStatus) { 0 } elseif ($lastNativeExitCode) { $lastNativeExitCode } else { 1 }

        ## 2. Report the status of the previous command to Atuin (atuin history end).

        if ($script:atuinHistoryId) {
            try {
                # The duration is not recorded in old PowerShell versions, let Atuin handle it. $null arguments are ignored.
                $duration = (Get-History -Count 1).Duration.Ticks * 100
                $durationArg = if ($duration) { "--duration=$duration" } else { $null }

                # Fire and forget the atuin history end command to avoid blocking the shell during a potential sync.
                $process = New-Object System.Diagnostics.Process
                $process.StartInfo.FileName = "atuin"
                $process.StartInfo.Arguments = "history end --exit=$exitCode $durationArg -- $script:atuinHistoryId"
                $process.StartInfo.UseShellExecute = $false
                $process.StartInfo.CreateNoWindow = $true
                $process.StartInfo.RedirectStandardInput = $true
                $process.StartInfo.RedirectStandardOutput = $true
                $process.StartInfo.RedirectStandardError = $true
                $process.Start() | Out-Null
                $process.StandardInput.Close()
                $process.BeginOutputReadLine()
                $process.BeginErrorReadLine()
            }
            catch {
                # Ignore errors to avoid breaking the shell.
                # An error would occur if the user removes atuin from the PATH, for instance.
            }
            finally {
                $script:atuinHistoryId = $null
            }
        }

        ## 3. Read the next command line to execute.

        # PSConsoleHostReadLine implementation from PSReadLine, adjusted to support old versions.
        Microsoft.PowerShell.Core\Set-StrictMode -Off

        $line = if ($script:hasExpectedReadLineOverload) {
            # When the overload we expect is available, we can pass $lastRunStatus to it.
            [Microsoft.PowerShell.PSConsoleReadLine]::ReadLine($Host.Runspace, $ExecutionContext, [System.Threading.CancellationToken]::None, $lastRunStatus)
        } else {
            # Either PSReadLine is older than v2.2.0-beta3, or maybe newer than we expect, so use the function from PSReadLine as-is.
            & $script:previousPSConsoleHostReadLine
        }

        ## 4. Report the next command line to Atuin (atuin history start).

        # PowerShell doesn't handle double quotes in native command line arguments the same way depending on its version,
        # and the value of $PSNativeCommandArgumentPassing - see the about_Parsing help page which explains the breaking changes.
        # This makes it unreliable, so we go through an environment variable, which should always be consistent across versions.
        try {
            $env:ATUIN_COMMAND_LINE = $line
            $script:atuinHistoryId = atuin history start --command-from-env
        }
        catch {
            # Ignore errors to avoid breaking the shell, see above.
        }
        finally {
            $env:ATUIN_COMMAND_LINE = $null
        }

        $global:LASTEXITCODE = $lastNativeExitCode
        return $line
    }

    function Invoke-AtuinSearch {
        param([string]$ExtraArgs = "")

        $previousOutputEncoding = [System.Console]::OutputEncoding
        $resultFile = New-TemporaryFile
        $suggestion = ""
        $errorOutput = ""

        try {
            [System.Console]::OutputEncoding = [System.Text.Encoding]::UTF8

            # Start-Process does some crazy stuff, just use the Process class directly to have more control.
            $process = New-Object System.Diagnostics.Process
            $process.StartInfo.FileName = "atuin"
            $process.StartInfo.Arguments = "search -i --result-file ""$resultFile"" $ExtraArgs"
            $process.StartInfo.UseShellExecute = $false
            $process.StartInfo.RedirectStandardError = $true
            $process.StartInfo.StandardErrorEncoding = [System.Text.Encoding]::UTF8
            $process.StartInfo.EnvironmentVariables["ATUIN_SHELL"] = "powershell"
            $process.StartInfo.EnvironmentVariables["ATUIN_QUERY"] = Get-CommandLine

            try {
                $process.Start() | Out-Null

                # A single stream is redirected, so we can read it synchronously, but we have to start reading it
                # before waiting for the process to exit, otherwise the buffer could fill up and cause a deadlock.
                $errorOutput = $process.StandardError.ReadToEnd().Trim()
                $process.WaitForExit()

                $suggestion = (Get-Content -Raw $resultFile -Encoding UTF8 | Out-String).Trim()
            }
            catch {
                $errorOutput = $_
            }

            if ($errorOutput) {
                Write-Host -ForegroundColor Red "Atuin error:"
                Write-Host -ForegroundColor DarkRed $errorOutput
            }

            # If no shell prompt offset is set, initialize it from the current prompt line count.
            if ($null -eq $env:ATUIN_POWERSHELL_PROMPT_OFFSET) {
                try {
                    $promptLines = (& $Function:prompt | Out-String | Measure-Object -Line).Lines
                    $env:ATUIN_POWERSHELL_PROMPT_OFFSET = -1 * ($promptLines - 1)
                }
                catch {
                    $env:ATUIN_POWERSHELL_PROMPT_OFFSET = 0
                }
            }

            # PSReadLine maintains its own cursor position, which will no longer be valid if Atuin scrolls the display in inline mode.
            # Fortunately, InvokePrompt can receive a new Y position and reset the internal state.
            $y = $Host.UI.RawUI.CursorPosition.Y + [int]$env:ATUIN_POWERSHELL_PROMPT_OFFSET
            $y = [System.Math]::Max([System.Math]::Min($y, [System.Console]::BufferHeight - 1), 0)
            [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt($null, $y)

            if ($suggestion -eq "") {
                # The previous input was already rendered by InvokePrompt
                return
            }

            $acceptPrefix = "__atuin_accept__:"

            if ( $suggestion.StartsWith($acceptPrefix)) {
                Set-CommandLine $suggestion.Substring($acceptPrefix.Length)
                [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
            } else {
                Set-CommandLine $suggestion
            }
        }
        finally {
            [System.Console]::OutputEncoding = $previousOutputEncoding
            Remove-Item $resultFile
        }
    }

    function Enable-AtuinSearchKeys {
        param([bool]$CtrlR = $true, [bool]$UpArrow = $true)

        if ($CtrlR) {
            Set-PSReadLineKeyHandler -Chord "Ctrl+r" -BriefDescription "Runs Atuin search" -ScriptBlock {
                Invoke-AtuinSearch
            }
        }

        if ($UpArrow) {
            Set-PSReadLineKeyHandler -Chord "UpArrow" -BriefDescription "Runs Atuin search" -ScriptBlock {
                $line = Get-CommandLine

                if (!$line.Contains("`n")) {
                    Invoke-AtuinSearch -ExtraArgs "--shell-up-key-binding"
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
