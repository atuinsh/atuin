# Atuin Docs (docs.atuin.sh)

Self-contained MkDocs Material site. Managed with [`uv`](https://docs.astral.sh/uv/)
and versioned with [`mike`](https://github.com/jimporter/mike).

## Local preview

```bash
cd docs
uv sync
uv run mkdocs serve   # live-reload single-version preview at http://localhost:8000
```

`mkdocs serve` is the everyday editing command — it does not show the version
selector. To preview the full multi-version site as it appears in production,
build versions into a scratch branch and use `mike serve` (see below).

## How versioning works

Production serves from the `gh-pages` branch, which `mike` populates:

- **Released versions** are published per `major.minor`. The newest release
  carries the `latest` alias and is the default landing page
  (`docs.atuin.sh/` → `docs.atuin.sh/latest/`).
- **`main`** is a separate version (`docs.atuin.sh/main/`) that always tracks
  the `main` branch. It is titled "main (unreleased)" and is **never** `latest`
  — it does not correspond to what `install.sh` installs.

Deploys are automated in `.github/workflows/docs-deploy.yml`:

| Trigger | Action |
| --- | --- |
| Push to `main` touching `docs/**` | redeploy the `main` version |
| Release tag `vX.Y.Z` | deploy `X.Y`, move `latest`, set root default |
| Prerelease tag `vX.Y.Z-...` | skipped |
| Manual `workflow_dispatch` | deploy `main`, or a release version if provided |

You normally never run `mike` by hand — CI does. To preview multiple versions
locally without pushing:

```bash
cd docs
uv run mike deploy --branch gh-pages-local --update-aliases 18.17 latest
uv run mike deploy --branch gh-pages-local main
uv run mike set-default --branch gh-pages-local latest
uv run mike serve --branch gh-pages-local
git branch -D gh-pages-local   # clean up
```
