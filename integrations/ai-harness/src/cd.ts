const unquote = (word: string): string => {
  const quote = word[0]
  return (quote === '"' || quote === "'") && word.endsWith(quote) && word.length >= 2
    ? word.slice(1, -1)
    : word
}

export const Cd = {
  /**
   * The directory a command's leading `cd` moves to, as written: `cd src && make` gives `src`.
   *
   * Undefined for a command that starts otherwise, and for `cd` alone, `cd -` or a target the shell
   * expands (`$VAR`, a backtick), whose directory depends on the shell's state.
   */
  leading: (command: string): string | undefined => {
    // `cd`'s own options (`-L`, `-P`, `-e`, `-@`) and the `--` that ends them come before the target.
    const match = /^\s*cd\s+(?:-[LPe@]+\s+)*(?:--\s+)?("[^"]*"|'[^']*'|[^\s;&|]+)\s*(?:$|&&|;|\|\||\n)/.exec(command)
    const target = match?.[1]
    if (target === undefined) return undefined
    const dir = unquote(target)
    return dir === '' || dir.startsWith('-') || /[$`]/.test(dir) ? undefined : dir
  },
}
