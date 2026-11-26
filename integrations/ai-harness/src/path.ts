export const Path = {
  /** `path` resolved against `base`, `~` against `home`, with `.` and `..` folded. */
  resolve: ({ base, path, home }: { base: string; path: string; home?: string }): string => {
    const expanded =
      home !== undefined && (path === '~' || path.startsWith('~/')) ? `${home}${path.slice(1)}` : path
    const joined = expanded.startsWith('/') ? expanded : `${base}/${expanded}`
    const parts = joined.split('/').reduce<string[]>((kept, part) => {
      if (part === '' || part === '.') return kept
      if (part === '..') return kept.slice(0, -1)
      return [...kept, part]
    }, [])
    return `/${parts.join('/')}`
  },

  /** Whether `path` is `root` or inside it. */
  isWithin: ({ path, root }: { path: string; root: string }): boolean =>
    path === root || path.startsWith(root.endsWith('/') ? root : `${root}/`),
}
