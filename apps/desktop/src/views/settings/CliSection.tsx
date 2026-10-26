import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { createExternalStore } from "../../lib/externalStore"
import {
  getCliInstallState,
  onCliInstallState,
  repairCliInstallation,
  removeCliInstallation,
  type CliInstallState,
} from "../../lib/cliInstallIpc"
import { SettingsRow } from "./SettingsSearchRows"

const cliStore = createExternalStore<{ state: CliInstallState | null; error: string | null }>({
  initial: { state: null, error: null },
  resetOnStop: true,
  subscribe: (set) => onCliInstallState((state) => set({ state, error: null })),
  load: async () => {
    try {
      return { state: await getCliInstallState(), error: null }
    } catch (error) {
      return { state: null, error: String(error) }
    }
  },
})

export function CliSection() {
  const { state, error } = useSyncExternalStore(cliStore.subscribe, cliStore.getSnapshot)
  const [busy, setBusy] = useState(false)

  async function act(action: () => Promise<CliInstallState>) {
    setBusy(true)
    try {
      cliStore.set({ state: await action(), error: null })
    } catch (error) {
      cliStore.set({ ...cliStore.getSnapshot(), error: String(error) })
    } finally {
      setBusy(false)
    }
  }

  return (
    <SectionGroup title="Command line">
      <Card>
        <SettingsRow
          searchId="cliCommand"
          description="The desktop app prepares antiburn-cli automatically. Integrations use its absolute path."
          trailing={
            <div className="flex gap-2">
              <PushButton
                disabled={busy || !state}
                onClick={() => void act(repairCliInstallation)}
              >
                Repair
              </PushButton>
              <PushButton
                disabled={busy || !state?.terminalInstalled}
                onClick={() => void act(removeCliInstallation)}
              >
                Remove terminal command
              </PushButton>
            </div>
          }
        >
          <div className="type-footnote mt-2 space-y-2 text-label-secondary" aria-live="polite">
            <p>
              {state
                ? state.runtime
                  ? `Runtime ready · CLI ${state.runtime.version}`
                  : "Runtime unavailable"
                : error
                  ? "CLI status unavailable"
                  : "Loading CLI status…"}
            </p>
            {state && (
              <>
                <p>
                  {state.terminalInstalled
                    ? "Terminal command installed"
                    : "Terminal command not installed"}
                </p>
                <p className="select-text break-all font-mono">{state.terminalPath}</p>
                <p>
                  Shell PATH is configured separately. Run the full path above, or add its
                  folder to your shell PATH. The app does not edit shell profiles.
                </p>
                <p>Removal keeps the integration runtime and stays in effect until Repair.</p>
              </>
            )}
            {(error || state?.diagnostic) && <p role="alert">{error || state?.diagnostic}</p>}
          </div>
        </SettingsRow>
      </Card>
    </SectionGroup>
  )
}
