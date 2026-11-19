-- Types for version 1 of Atuin's config files.

--- A thread, as of where it started.
type Thread = {
	--- The harness's name: `claude-code`, `codex`, `copilot`, `opencode`, `pi`, `omp`, `grok`,
	--- `amp`, `kiro`, `hermes`, or `unknown`.
	harness: string,
	--- The harness's own id for the thread.
	id: string,
	--- The directory the thread started in, if it named one.
	dir: string?,
	--- The directory of the nearest project's config file at or above `dir`, if there's one.
	workspace: string?,
}

--- A command that ran.
type Command = {
	command: string,
	--- The directory it ran in.
	dir: string,
	exit: number,
	--- How long it ran, in nanoseconds.
	duration: number,
	author: string,
	--- Whether an agent ran it.
	agent: boolean,
	--- Why the agent that ran it said it did.
	intent: string?,
	shell: string?,
	--- As a thread's.
	workspace: string?,
}

--- Whether something goes up: for all of it, or for each one the function takes.
type Upload<T> = boolean | (T) -> boolean

--- What a config file sets.
type AtuinConfig = {
	--- Which organizations are owners of this project.
	---
	--- Any members of this organization will receive all threads in this project. Non-members will
	--- not have any of their data uploaded.
	orgs: { string }?,
  --- Configure what you want to be uploaded.
	upload: {
		--- Upload threads to the Hub?
		threads: Upload<Thread>?,
    --- Upload history to the Hub?
		history: Upload<Command>?,
	}?,
}

declare atuin: {
	--- The file's config:
	---
	--- ```lua
	--- -- atuin:version=1
	--- atuin.config = {
	--- 	orgs = { "acme" },
	--- }
	--- ```
	config: AtuinConfig,
}
