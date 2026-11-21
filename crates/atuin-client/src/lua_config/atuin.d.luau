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
		--- @deprecated v18.25.2 -- The Hub takes threads only, so this decides nothing.
		history: Upload<any>?,
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
