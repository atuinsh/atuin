-- Types for version 1 of Atuin's config files.

--- What a config file sets.
type AtuinConfig = {
	--- Which organizations are owners of this project.
	---
	--- Any members of this organization will receive all threads in this project. Non-members will
	--- not have any of their data uploaded.
	orgs: { string }?,
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
