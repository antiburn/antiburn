import { useState } from "react"

interface AccountIdentity {
  key: string
  provider: string
}

interface AccountNumberState {
  numbers: ReadonlyMap<string, number>
  nextByProvider: ReadonlyMap<string, number>
}

const EMPTY_STATE: AccountNumberState = {
  numbers: new Map(),
  nextByProvider: new Map(),
}

function addAccounts(
  state: AccountNumberState,
  accounts: readonly AccountIdentity[],
): AccountNumberState {
  if (accounts.every(({ key }) => state.numbers.has(key))) return state

  const numbers = new Map(state.numbers)
  const nextByProvider = new Map(state.nextByProvider)
  for (const { key, provider } of accounts) {
    if (numbers.has(key)) continue
    const number = nextByProvider.get(provider) ?? 1
    numbers.set(key, number)
    nextByProvider.set(provider, number + 1)
  }
  return { numbers, nextByProvider }
}

/** Keep account numbers fixed for the lifetime of the current surface. */
export function useStableAccountNumbers(
  accounts: readonly AccountIdentity[],
): ReadonlyMap<string, number> {
  const [state, setState] = useState(() => addAccounts(EMPTY_STATE, accounts))
  const current = addAccounts(state, accounts)
  if (current !== state) setState(current)
  return current.numbers
}
