import * as v from 'valibot'

export type Outcome<T> = { isOk: true; value: T } | { isOk: false; reason: string }

export const Outcome = {
  ok: <T>(value: T): Outcome<T> => ({ isOk: true, value }),
  failed: <T = never>(reason: string): Outcome<T> => ({ isOk: false, reason }),
  schema: <S extends v.GenericSchema>(value: S) =>
    v.variant('isOk', [
      v.object({ isOk: v.literal(true), value }),
      v.object({ isOk: v.literal(false), reason: v.string() }),
    ]),
}
