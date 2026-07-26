"""Keep llms.txt English-only when mkdocs-static-i18n is enabled.

mkdocs-static-i18n builds each extra locale by calling mkdocs' `build()` again
with the same config object, so every plugin instance is reused. mkdocs-llmstxt
keys its page state on `src_uri`, which is identical across locales for any
page served by the default-language fallback, and derives each entry's URL from
`dest_uri`, which is not. Each nested build therefore overwrites the English
state with locale-prefixed URLs, and whichever locale is configured last wins:
all 51 curated entries in llms.txt end up pointing at /ru/. It reproduces with
zero translated files, so it is not a translation problem.

Neither plugin is obviously at fault -- one reuses instances across builds, the
other holds mutable state across them -- and there is no upstream issue for the
combination. Skipping llmstxt's stateful hooks outside the default-language
build leaves the English pass intact and produces exactly one English llms.txt,
which is the semantics we want anyway.

If either plugin is removed from mkdocs.yml this hook does nothing. If their
internals move, `_locale_of` raises at build time rather than silently letting
the corruption back in.
"""

# Hooks that read or mutate mkdocs-llmstxt's cross-build page state, mapped to
# what each must return when skipped: on_files passes the file collection
# through, on_page_content passes the rendered HTML, on_post_build returns None.
_GUARDED_HOOKS = {
    "on_files": lambda *args, **kwargs: args[0],
    "on_page_content": lambda *args, **kwargs: args[0],
    "on_post_build": lambda *args, **kwargs: None,
}


def _locale_of(i18n_plugin):
    """Return (current locale, default locale) from the i18n plugin instance."""
    return i18n_plugin.current_language, i18n_plugin.default_language


def _guarded(original, i18n_plugin, skip):
    def wrapper(*args, **kwargs):
        current, default = _locale_of(i18n_plugin)
        if current is not None and current != default:
            return skip(*args, **kwargs)
        return original(*args, **kwargs)

    wrapper.__i18n_guarded__ = True
    return wrapper


def on_config(config):
    i18n_plugin = config.plugins.get("i18n")
    llmstxt_plugin = config.plugins.get("llmstxt")
    if i18n_plugin is None or llmstxt_plugin is None:
        return config

    # mkdocs binds every plugin method into `plugins.events` when the plugin is
    # loaded, so replacing the attribute on the instance would be ignored --
    # the already-registered callable is what run_event() dispatches to.
    for hook_name, skip in _GUARDED_HOOKS.items():
        event_name = hook_name.removeprefix("on_")
        registered = config.plugins.events[event_name]
        for index, method in enumerate(registered):
            if getattr(method, "__self__", None) is not llmstxt_plugin:
                continue
            # mkdocs re-runs on_config for every nested locale build; wrap once.
            if getattr(method, "__i18n_guarded__", False):
                continue
            registered[index] = _guarded(method, i18n_plugin, skip)

    return config
