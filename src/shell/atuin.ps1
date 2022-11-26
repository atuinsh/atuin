$env:ATUIN_SESSION = (atuin uuid)
#set your fzf path here
$script:FZF_LOCATION = "C:\ProgramData\chocolatey\bin\fzf.exe"

#TODO SAVE AND RESTORE : RUST_LOG and FZF_DEFAULT_COMMAND

#Based on starship powershell script
function global:prompt {
    $origDollarQuestion = $global:?
    $origLastExitCode = $global:LASTEXITCODE

    # We start from the premise that the command executed correctly, which covers also the fresh console.
    $lastExitCodeForPrompt = 0
    if ($lastCmd = Get-History -Count 1) {
        # In case we have a False on the Dollar hook, we know there's an error.
        if (-not $origDollarQuestion) {
            # We retrieve the InvocationInfo from the most recent error using $global:error[0]
            $lastCmdletError = try { $global:error[0] |  Where-Object { $_ -ne $null } | Select-Object -ExpandProperty InvocationInfo } catch { $null }
            # We check if the last command executed matches the line that caused the last error, in which case we know
            # it was an internal Powershell command, otherwise, there MUST be an error code.
            $lastExitCodeForPrompt = if ($null -ne $lastCmdletError -and $lastCmd.CommandLine -eq $lastCmdletError.Line) { 1 } else { $origLastExitCode }
        }
    }

    if ( (-not [string]::IsNullOrEmpty($lastCmd.CommandLine)) -and ($lastCmd.CommandLine -eq $script:atuincmd)) {
        $env:RUST_LOG = 'error'; atuin history end --exit "${lastExitCodeForPrompt}" -- "$script:atuinid"; $env:RUST_LOG = ''
    }

    # Set the number of extra lines in the prompt for PSReadLine prompt redraw.
    # Set-PSReadLineOption -ExtraPromptLineCount ($promptText.Split("`n").Length - 1)

    # Propagate the original $LASTEXITCODE from before the prompt function was invoked.
    $global:LASTEXITCODE = $origLastExitCode

    # Propagate the original $? automatic variable value from before the prompt function was invoked.
    #
    # $? is a read-only or constant variable so we can't directly override it.
    # In order to propagate up its original boolean value we will take an action
    # which will produce the desired value.
    #
    # This has to be the very last thing that happens in the prompt function
    # since every PowerShell command sets the $? variable.
    if ($global:? -ne $origDollarQuestion) {
        if ($origDollarQuestion) {
            # Simple command which will execute successfully and set $? = True without any other side affects.
            1 + 1
        }
        else {
            # Write-Error will set $? to False.
            # ErrorAction Ignore will prevent the error from being added to the $Error collection.
            Write-Error '' -ErrorAction 'Ignore'
        }
    }

}

#https://github.com/kelleyma49/PSFzf/blob/0364b14bbd5013a37348fe6681c4980195515ab2/PSFzf.Functions.ps1#L325
function SearchWithFZF() {
    $ATUIN_PREFIX = "atuin search --cmd-only"
    $INITIAL_QUERY = ""
    $env:FZF_DEFAULT_COMMAND = ''
    try {
        $sleepCmd = ''
        $env:FZF_DEFAULT_COMMAND = "$ATUIN_PREFIX  ""$INITIAL_QUERY"""

        & $script:FZF_LOCATION --ansi `
            --disabled --query `"$INITIAL_QUERY`" `
            --bind "change:reload:$sleepCmd $ATUIN_PREFIX  {q} || cd . " | `
            ForEach-Object { $results += $_ }

        if (-not [string]::IsNullOrEmpty($results)) {
            return $results
        }
        return ""
    }
    catch {
        Write-Error "Error occurred: $_"
    }
    finally {
        # todo restore $env:FZF_DEFAULT_COMMAND
    }
}

#https://github.com/kelleyma49/PSFzf/blob/73fd091883b26866be8d2c4acdfdbd1b11baf45f/PSFzf.Base.ps1#L125
function InvokePromptHack() {
    $previousOutputEncoding = [Console]::OutputEncoding
    [Console]::OutputEncoding = [Text.Encoding]::UTF8

    try {
        [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt()
    }
    finally {
        [Console]::OutputEncoding = $previousOutputEncoding
    }
}

#https://github.com/kelleyma49/PSFzf/blob/de453b0ab8ec52255a308ddf559b330defb3b842/PSFzf.Git.ps1#L114
function Update-CmdLine($result) {
    InvokePromptHack
    if ($result.Length -gt 0) {
        $result = $result -join " "
        [Microsoft.PowerShell.PSConsoleReadLine]::Insert($result)
    }
}

# https://github.com/PowerShell/PowerShell/issues/15271
# Override the PSReadLine Enter key handler in order to inject
# custom logic just before submitting a command.
function onEnter() {
    # Get the text of the comamnd being submitted.
    try {
        $line = $null; [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref] $line, [ref] $null)
        $script:atuincmd = $line
        $script:atuinid = (atuin history start -- "$line")

    }
    finally {
        # Submit the command.
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
}

Set-PSReadLineKeyHandler Enter -ScriptBlock { onEnter }
Set-PSReadlineKeyHandler -Key Ctrl+r -ScriptBlock { Update-CmdLine $(SearchWithFZF) }
