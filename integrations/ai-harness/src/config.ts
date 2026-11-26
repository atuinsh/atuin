/** The mod's settings, each read as given or its default when left blank. */
export type Config = { atuinPath: string }

const text = (value: string, fallback: string): string => value.trim() === '' ? fallback : value.trim()

export const configOf = ({ atuinPath }: Config): Config => ({
  atuinPath: text(atuinPath, 'atuin'),
})
