unrecognized-subcommand =
    unrecognized subcommand '{ $subcommand }' and no executable named '{ $bin }' found in your PATH
cmd-contributors = List the people who have contributed to Atuin
arg-gen-completions-shell = Set the shell for generating completions
arg-gen-completions-out-dir = Set the output directory
output-search-disabled = output capture is disabled. enable [output] in your config to search command output.
output-search-empty-query = blank query provided. please run 'atuin output search --help'
daemon-remove-socket-failed = failed to remove daemon socket { $path }: { $source }
hook-malformed-json = hook payload is not valid JSON at line { $line }, column { $column }
output-search-connect-failed = could not connect to the daemon
output-search-daemon-failed = the daemon failed to search command output
output-search-load-history-failed = could not load history from the local database
output-search-write-failed = could not write search results
log-otel-collector-failed = failed to build the otel collector: { $error }
log-subscriber-init-failed = failed to initialize subscriber: { $error }
log-file-name-not-utf8 = log file name must be utf-8
otel-not-compiled = this build of atuin has no OpenTelemetry support: rebuild with the `profiling-traced` feature (e.g. `cargo build-traced`) to export `ATUIN_OTEL` traces
otel-url-not-utf8 = the given ATUIN_OTEL URL does not appear to be a utf-8 string
otel-url-invalid = the given ATUIN_OTEL failed to parse as URL: { $error }
otel-url-not-http = the given ATUIN_OTEL URL does not appear to be an HTTP(s) URL
otel-exporter-build-failed = failed to construct the exporter: { $error }
account-run-logout = Run 'atuin logout' to log out.
account-not-migrated-note = Note: Your account has not been fully migrated to Atuin Hub.
account-not-logged-in = You are not logged in
account-provide-password = please provide a password
prompt-username = Please enter username
prompt-email = Please enter email
prompt-password = Please enter password
prompt-current-password = Please enter the current password
prompt-new-password = Please enter the new password
prompt-two-factor-code = Please enter two-factor code
prompt-key-or-existing = Please enter encryption key [blank to use existing key file]
prompt-key-or-logout = Please enter encryption key [blank to log out and cancel]
account-hub-authenticated = You are already authenticated with Atuin Hub.
login-legacy-logged-in = You are logged in to your sync server.
login-upgrading-legacy = You have a legacy sync session. Continuing login to upgrade to full Hub authentication.
account-not-migrated-hint = Sync will continue to work, but you can visit hub.atuin.sh to create a Hub account and link it to your existing CLI account.
login-key-generate-failed = could not load or generate encryption key
login-success = Successfully authenticated.
login-legacy-unexpected-2fa = unexpected two-factor requirement from legacy server
login-legacy-success = Logged in!
account-hub-open-url = Open this URL to authenticate with Atuin Hub:
login-key-important = IMPORTANT
login-key-same-everywhere = If you are already logged in on another machine, you must ensure that the key you use here is the same as the key you used there.
login-key-find = You can find your key by running 'atuin key' on the other machine.
login-key-secret = Do not share this key with anyone.
login-key-read-more = Read more here: { $url }
login-no-key-provided = No encryption key provided
login-no-key-found = No key provided and no existing key file found. Please use 'atuin key' on your other machine, or recover your key from a backup
login-key-file-invalid = The key in existing key file at '{ $path }' is invalid
login-key-try-again = { $error }. Please try again.
login-reencrypting = Re-encrypting local store with new key
login-writing-key = Writing new key
login-key-load-failed = could not load encryption key for verification
login-key-mismatch = The encryption key on this machine does not match the data on the server.
login-key-find-correct = You can find the correct key by running 'atuin key' on a machine that already syncs successfully.
login-wrong-key-title = Wrong encryption key
login-wrong-key-body =
    The encryption key on this machine does not match the data on the server. You have been logged out.

    To fix this, find your existing key by running `atuin key` on a machine that already syncs successfully, then run `atuin login` again here with that key.
register-legacy-already = You are already logged in.
register-has-legacy-session = You already have a sync session. Run 'atuin login' to upgrade to full Hub authentication.
register-logout-first = Run 'atuin logout' first if you want to register a new account.
register-headless-incomplete = Username, password, and email are all required for headless registration. Continuing with interactive registration.
register-unexpected-2fa = unexpected two-factor requirement during registration
register-success-key = Registration successful! Please make a note of your key (run 'atuin key') and keep it safe.
register-key-warning = You will need it to log in on other devices, and we cannot help recover it if you lose it.
register-legacy-start = Registering for an Atuin Sync account
link-no-cli-session = No CLI session found. Please log in first with 'atuin login'.
link-both-sessions = Found both Hub and CLI sessions. Linking accounts...
link-hub-login-first = Found CLI session but no Hub session. Logging in to Hub first...
link-hub-complete = Hub authentication complete.
link-success = Successfully linked CLI account to Hub.
delete-provide-password = please provide your password
delete-success = Your account is deleted
change-password-provide-current = please provide the current password
change-password-provide-new = please provide a new password
change-password-success = Account password successfully changed!
sync-key-load-failed = could not load encryption key
sync-key-invalid = invalid key
sync-up-down = { $uploaded }/{ $downloaded } up/down to record store
sync-history-mismatch = { $index } in history index, but { $store } in history store
sync-store-init = Running automatic history store init...
sync-rerun = Re-running sync due to new records locally
sync-complete = Sync complete! { $count } items in history database, force: { $force }
sync-status-not-logged-in = You are not logged in to a sync server - cannot show sync status
sync-status-version = Atuin v{ $version } - Build rev { $sha }
sync-status-local = [Local]
sync-status-frequency = Sync frequency: { $frequency }
sync-status-last-sync = Last sync: { $time }
sync-status-remote = [Remote]
sync-status-address = Address: { $address }
sync-status-username = Username: { $username }
cmd-search = Interactive history search
cmd-stats = Calculate statistics for your history
cmd-account = Manage your sync account
cmd-setup = Setup Atuin features
cmd-init = Print Atuin's shell init script
cmd-doctor = Run the doctor to check for common issues
cmd-update = Update atuin to the latest version on your release channel
cmd-hook = Manage AI-agent shell hooks
cmd-mcp = Start an MCP server exposing history search to AI tools (stdio)
cmd-wrapped = Show a fun, year-in-review recap of your shell history
arg-wrapped-year = Year to recap (defaults to last year)
cmd-default-config = Print the default atuin configuration (config.toml)
cmd-info = Information about Atuin data locations and ENV vars
cmd-daemon = {"*"}Experimental* Manage the background daemon
cmd-__internal_ = We want to exclude the `__internal` subcommand from Clap's `infer_subcommands`; otherwise, a user could access it simply by typing `atuin _`. However, Clap has no way to disable `infer_subcommands` for a single command. As a workaround, we define a dummy command with the same name but with an extra understore, which forces `__internal` to be typed out in entirety, since any prefix of the name would be ambiguous
cmd-account-register = Register a new account
cmd-account-delete = Delete your account, and all synced data
cmd-account-change-password = Change your password
cmd-account-link = Link your CLI sync account to your Hub account
arg-account-login-key = The encryption key for your account. Falls back to `ATUIN_ENCRYPTION_KEY`, then a prompt
arg-totp-code = The two-factor authentication code for your account, if any
cmd-config-get = Get a configuration value from your config.toml file or after defaults and overrides are applied
cmd-config-set = Set a configuration value in your config.toml file
cmd-config-enable = Enable a feature, along with everything it depends on
cmd-config-print = Print all configuration values from your config.toml file in TOML format
    .long = 
        Print all configuration values from your config.toml file in TOML format

        If a key is provided, only print the value of that key and all its children
arg-config-get-key = The configuration key to get
arg-config-get-resolved = Print the value after defaults and overrides are applied
arg-config-get-verbose = Print both the config file value and the resolved value
arg-config-set-key = The configuration key to set
arg-config-set-value = The value to set
arg-config-set-the-type = Store value as an explicit type
value-config-set-the-type-auto = Automatically determine the type of the value
value-config-set-the-type-string = Store value as a string
value-config-set-the-type-boolean = Store value as a boolean
value-config-set-the-type-integer = Store value as an integer
value-config-set-the-type-float = Store the value as a float
arg-config-enable-feature = The feature to enable
value-config-enable-feature-daemon = Run the daemon, autostart it, and use it for search
value-config-enable-feature-output-capture = Capture command output, via the daemon and the pty proxy
arg-config-print-key = Print the value of a specific key and all its children
arg-daemon-daemonize = Internal flag for daemonization
arg-daemon-show-logs = Also write daemon logs to the console (useful for debugging)
cmd-daemon-start = Start the daemon server
arg-daemon-start-force = Force start: kill existing daemon process and reset the socket
cmd-daemon-status = Show the daemon's current status
cmd-daemon-stop = Stop the daemon gracefully
cmd-daemon-restart = Restart the daemon (stop, then start in background)
value-dotfiles-alias-list-sort-by-name = Sort by alias name
value-dotfiles-alias-list-sort-by-value = Sort by alias value
cmd-dotfiles-alias-list = List all aliases
arg-dotfiles-list-sort-by = Sort results by field
arg-dotfiles-list-reverse = Sort in reverse (descending) order
arg-dotfiles-alias-list-name = Filter aliases by name (substring match)
arg-dotfiles-alias-list-value = Filter aliases by value (substring match)
value-dotfiles-var-list-sort-by-name = Sort by variable name
value-dotfiles-var-list-sort-by-value = Sort by variable value
cmd-dotfiles-var-list = List all variables
arg-dotfiles-var-list-name = Filter variables by name (substring match)
arg-dotfiles-var-list-value = Filter variables by value (substring match)
arg-dotfiles-var-list-exports-only = Show only exported variables
arg-dotfiles-var-list-shell-only = Show only non-exported (shell) variables
cmd-history-start = Begins a new command in the history
arg-history-start-cmd-env = Collects the command from the `ATUIN_COMMAND_LINE` environment variable, which does not need escaping and is more compatible between OS and shells
arg-history-start-author = Author of this command, eg `ellie`, `claude`, or `copilot`
arg-history-start-author-kind = Whether a human or an AI agent ran this command
    .long = 
        Whether a human or an AI agent ran this command

        {"["}`Option::None`] will cause us to perform a best-guess effort.
arg-history-start-intent = Optional intent/rationale for running this command
arg-history-hook = Passed by shell hooks; this flag disables logging to avoid corrupting the terminal and to minimize the amount of time the command takes to run
cmd-history-end = Finishes a new command in the history (adds time, exit code)
cmd-history-tail = Stream history events from the daemon as they are received
cmd-history-list = List all items in history
arg-print0 = Terminate the output with a null, for better multiline support
arg-history-timezone = Display the command time in another timezone other than the configured default.
    .long = 
        Display the command time in another timezone other than the configured default.

        This option takes one of the following kinds of values:

        - the special value "local" (or "l") which refers to the system time zone
        - an offset from UTC (e.g. "+9", "-2:30")
arg-history-list-format = Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}author{"}"}, {"{"}intent{"}"}, {"{"}exit{"}"}, {"{"}time{"}"}, {"{"}session{"}"}, and {"{"}uuid{"}"}
    .long = 
        Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}author{"}"}, {"{"}intent{"}"}, {"{"}exit{"}"}, {"{"}time{"}"}, {"{"}session{"}"}, and {"{"}uuid{"}"}

        Example: --format "{"{"}time{"}"} - [{"{"}duration{"}"}] - {"{"}directory{"}"}$\t{"{"}command{"}"}"
cmd-history-last = Get the last command that was run
arg-history-last-format = Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}author{"}"}, {"{"}intent{"}"}, {"{"}time{"}"}, {"{"}session{"}"}, {"{"}uuid{"}"} and {"{"}relativetime{"}"}
    .long = 
        Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}author{"}"}, {"{"}intent{"}"}, {"{"}time{"}"}, {"{"}session{"}"}, {"{"}uuid{"}"} and {"{"}relativetime{"}"}.

        Example: --format "{"{"}time{"}"} - [{"{"}duration{"}"}] - {"{"}directory{"}"}$\t{"{"}command{"}"}"
cmd-history-prune = Delete history entries matching the configured exclusion filters
arg-history-dry-run = List matching history lines without performing the actual deletion
cmd-history-dedup = Delete duplicate history entries (that have the same command, cwd and hostname)
arg-history-dedup-before = Only delete results added before this date, read in the configured timezone unless it carries an explicit offset
arg-history-dedup-dupkeep = How many recent duplicates to keep
cmd-hook-install = Install hooks for an AI agent to capture commands in atuin history
arg-hook-install-agent = Agent to install hooks for (e.g., "claude-code")
arg-hook-agent = Which agent's hook format to parse (e.g., "claude-code")
cmd-import-auto = Import history for the current shell
cmd-import-shell = Import history from the { $shell } history file
cmd-import-zsh-hist-db = Import history from the zsh-histdb database
cmd-import-nu-hist-db = Import history from the nu history database
cmd-import-xonsh = Import history from xonsh json files
cmd-import-xonsh-sqlite = Import history from xonsh sqlite db
arg-init-disable-ctrl-r = Disable the binding of CTRL-R to atuin
arg-init-disable-up-arrow = Disable the binding of the Up Arrow key to atuin
arg-init-disable-ai = Disable the binding of ? to Atuin AI
value-init-shell-zsh = Zsh setup
value-init-shell-bash = Bash setup
value-init-shell-fish = Fish setup
value-init-shell-nu = Nu setup
value-init-shell-xonsh = Xonsh setup
value-init-shell-powershell = PowerShell setup
cmd-__internal-pty-proxy-active = Check whether the current terminal belongs to a live PTY proxy
    .long = 
        Check whether the current terminal belongs to a live PTY proxy.

        Prints `0` or `1` to stdout. This command is used by shell hooks to determine whether the PTY proxy is in use.
cmd-kv-set = Set a key-value pair
arg-kv-set-key = Key to set
arg-kv-set-value = Value to store (reads from stdin if not provided)
arg-kv-namespace = Namespace for the key-value pair
cmd-kv-delete = Delete one or more key-value pairs
arg-kv-delete-keys = Keys to delete
cmd-kv-get = Retrieve a saved value
arg-kv-get-key = Key to retrieve
cmd-kv-list = List all keys in a namespace, or in all namespaces
arg-kv-list-namespace = Namespace to list keys from
arg-kv-list-all-namespaces = List all keys in all namespaces
cmd-kv-rebuild = Rebuild the KV store
cmd-output-search = Full-text search over captured command output
value-output-search-style-json = A single JSON array of match objects
value-output-search-style-ndjson = Newline-delimited JSON: one match object per line
arg-output-search-query = Words to search for; all must appear. Use `--` before a query that starts with `-`
arg-output-search-limit = Maximum number of matches to return
arg-output-search-context = Show only the matching lines, with this many lines of context on either side; without it, each match's whole output
arg-output-search-style = How matches are rendered
arg-scripts-new-last = Use the last command as the script content
    .long = 
        Use the last command as the script content

        Optionally specify a number to use the last N commands
arg-scripts-new-no-edit = Skip opening editor when using --last
arg-scripts-run-var = Specify template variables in the format KEY=VALUE
    .long = 
        Specify template variables in the format KEY=VALUE

        Example: -v name=John -v greeting="Hello there"
arg-scripts-get-script = Display only the executable script with shebang
arg-scripts-edit-tags = Replace all existing tags with these new tags
arg-scripts-edit-no-tags = Remove all tags from the script
arg-scripts-edit-rename = Rename the script
arg-scripts-edit-no-edit = Skip opening editor
arg-search-cwd = Filter search result by directory
arg-search-exclude-cwd = Exclude directory from results
arg-search-exit = Filter by exit code; repeat to include any of the given codes
arg-search-exclude-exit = Exclude results with this exit code; repeat to exclude multiple codes
arg-search-before = Only include results added before this date
    .long = 
        Only include results added before this date.

        Read in the timezone from `--timezone` (or the configured one) unless it carries an explicit offset; relative phrases like "yesterday 3pm" are anchored there too.
arg-search-after = Only include results after this date; see `--before` for how it is interpreted
arg-search-limit = How many entries to return at most
arg-search-offset = Offset from the start of the results
arg-search-interactive = Open interactive search UI
arg-search-filter-mode = Allow overriding filter mode over config
arg-search-search-mode = Allow overriding search mode over config
    .long = 
        Allow overriding search mode over config

        Note: for non-interactive searches, "daemon-fuzzy" behaves like "fuzzy". "skim" used to behave like "fuzzy" in non-interactive searches too; it has since been removed but is still accepted here as an alias of "fuzzy".
arg-search-shell-up-key-binding = Marker argument used to inform atuin that it was invoked from a shell up-key binding (hidden from help to avoid confusion)
arg-search-keymap-mode = Notify the keymap at the shell's side
arg-search-human = Use human-readable formatting for time
arg-cmd-only = Show only the text of the command
arg-search-delete = Delete anything matching this query. Will not print out the match
arg-search-delete-it-all = Delete EVERYTHING!
arg-search-reverse = Reverse the order of results, oldest first
arg-search-timezone = 
    Timezone to display command times in and to interpret `--before`/`--after` in, instead
    of the configured default.
    .long = 
        Timezone to display command times in and to interpret `--before`/`--after` in, instead
        of the configured default.

        This option takes one of the following kinds of values:

        - the special value "local" (or "l") which refers to the system time zone
        - an offset from UTC (e.g. "+9", "-2:30")
arg-search-format = Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}time{"}"}, {"{"}exit{"}"} and {"{"}relativetime{"}"}
    .long = 
        Available variables: {"{"}command{"}"}, {"{"}directory{"}"}, {"{"}duration{"}"}, {"{"}user{"}"}, {"{"}host{"}"}, {"{"}time{"}"}, {"{"}exit{"}"} and {"{"}relativetime{"}"}.

        Example: --format "{"{"}time{"}"} - [{"{"}duration{"}"}] - {"{"}directory{"}"}$\t{"{"}command{"}"}"
arg-search-inline-height = Set the maximum number of lines Atuin's interface should take up
arg-search-author = Filter by author. Supports $all-user (non-agents), $all-agent, or literal names
    .long = 
        Filter by author. Supports $all-user (non-agents), $all-agent, or literal names.

        Can be specified multiple times.
arg-search-include-duplicates = Include duplicate commands in the output (non-interactive only)
arg-search-result-file = File name to write the result to (hidden from help as this is meant to be used from a script)
arg-search-shell = Filter by the shell that was used to run the command
    .long = 
        Filter by the shell that was used to run the command

        If passed multiple times, commands from any of the shells will be shown.

        `--shell ""` will include commands for which the shell is unknown.
arg-stats-period = Compute statistics for the specified period, leave blank for statistics since the beginning
    .long = Compute statistics for the specified period, leave blank for statistics since the beginning. See [this]({ $url }) for more details.
arg-stats-count = How many top commands to list
arg-stats-ngram-size = The number of consecutive commands to consider
arg-stats-filter-mode = Filter commands by scope [global, host, session, directory, workspace]
cmd-store-status = Print the current status of the record store
cmd-store-rebuild = Rebuild a store (eg atuin store rebuild history)
cmd-store-rekey = Re-encrypt the store with a new key (potential for data loss!)
cmd-store-compact = Rewrite every record in the compact on-disk encoding and reclaim space
cmd-store-purge = Delete all records in the store that cannot be decrypted with the current key
cmd-store-verify = Verify that all records in the store can be decrypted with the current key
cmd-store-push = Push all records to the remote sync server (one way sync)
cmd-store-pull = Pull records from the remote sync server (one way sync)
arg-store-pull-force = Force push records
    .long = 
        Force push records

        This will first wipe the local store, and then download all records from the remote
arg-store-pull-page = Page Size
    .long = 
        Page Size

        How many records to download at once. Defaults to 100
arg-store-tag = The tag to push (eg, 'history'). Defaults to all tags
arg-store-push-host = The host to push, in the form of a UUID host ID. Defaults to the current host
arg-store-push-force = Force push records
    .long = 
        Force push records

        This will override both host and tag, to be all hosts and all tags. First clear the remote store, then upload all of the local store
arg-store-push-page = Page Size
    .long = 
        Page Size

        How many records to upload at once. Defaults to 100
arg-store-rekey-key = The new key to use for encryption. Omit for a randomly-generated key
cmd-sync = Sync with the configured server
arg-sync-force = Force re-download everything
cmd-login = Login to the configured server
cmd-logout = Log out
cmd-register = Register with the configured server
cmd-key = Print the encryption key for transfer to another machine
arg-key-base64 = Switch to base64 output of the key
cmd-status = Display the sync status
arg-update-check = Check whether an update is available without installing it
arg-update-version = Update (or roll back) to a specific version, e.g. "18.9.0" or "18.9.0-nightly.1", instead of the channel's latest release
cmd-pty-proxy = PTY proxy for atuin
cmd-uuid = Generate a UUID
cmd-gen-completions = Generate shell completions
cmd-atuin = Magical shell history
arg-password = Your password, or `-` to read it from stdin. Falls back to `ATUIN_PASSWORD`, then a prompt
login-totp-required = A two-factor code is required. Pass it with --totp-code
account-password-stdin-failed = failed to read password from stdin
