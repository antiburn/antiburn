import {
  Circle,
  TriangleAlert,
  LoaderCircle,
  Stethoscope,
  Wrench,
  Copy,
  Check,
} from "lucide-react"
import { useCallback, useRef, useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Disclosure } from "../../components/ui/Disclosure"
import { PushButton } from "../../components/ui/PushButton"
import { Pane } from "../../components/ui/Pane"
import { writeClipboardText } from "../../lib/clipboard"
import { createExternalStore } from "../../lib/externalStore"
import {
  getCliInstallState,
  onCliInstallState,
  repairCliInstallation,
  testCliInstallation,
  type CliInstallState,
} from "../../lib/cliInstallIpc"
import { SettingsRow, SettingsFocusTarget } from "./SettingsSearchRows"

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

type TestResult = { key: string; sha256: string; error: string | null }
type Completion = { key: string; message: string }

function statusKey(state: CliInstallState | null, error: string | null) {
  return JSON.stringify([state, error])
}

export function CliSection() {
  const { state, error } = useSyncExternalStore(cliStore.subscribe, cliStore.getSnapshot)
  const [busy, setBusy] = useState<"test" | "repair" | "copy" | null>(null)
  const technicalDetails = useRef<HTMLDivElement>(null)
  const operationPending = useRef(false)
  const copyTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const copyLive = useRef(false)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const key = statusKey(state, error)
  const [observedKey, setObservedKey] = useState(key)
  const [testResult, setTestResult] = useState<TestResult | null>(null)
  const [copyStatus, setCopyStatus] = useState<Completion | null>(null)
  const [completion, setCompletion] = useState<Completion | null>(null)
  const bindCopyLifetime = useCallback((node: HTMLButtonElement | null) => {
    if (!node) return
    copyLive.current = true
    return () => {
      copyLive.current = false
      clearTimeout(copyTimer.current)
      setCopyStatus(null)
    }
  }, [])
  const copied = copyStatus?.key === key && copyStatus.message === "Copied!"
  if (observedKey !== key) {
    setObservedKey(key)
    setTestResult((value) => (value?.key === key ? value : null))
    setCopyStatus((value) => (value?.key === key ? value : null))
    setCompletion((value) => (value?.key === key ? value : null))
  }
  const result =
    testResult?.key === key && testResult.sha256 === state?.runtime?.sha256 ? testResult : null
  const detail = error || state?.diagnostic || result?.error
  const needsRepair = Boolean(error || state?.diagnostic || (state && !state.runtime))
  const StatusIcon = needsRepair ? TriangleAlert : Circle
  const status =
    busy === "test"
      ? "Testing CLI…"
      : busy === "repair"
        ? "Repairing CLI…"
        : !state
          ? error
            ? "CLI status unavailable. Try again."
            : "Loading CLI status…"
          : result?.error
            ? "Couldn’t start the CLI. Try Repair."
            : !state.runtime
              ? "CLI unavailable. Try Repair."
              : result
                ? "Test passed"
                : state.diagnostic
                  ? "CLI needs attention. Open technical details for troubleshooting."
                  : "Installed"
  const inlineStatus = Boolean(state?.runtime && !needsRepair)
  const statusTone = needsRepair ? "text-system-orange" : "text-label-secondary"

  async function test() {
    const runtime = state?.runtime
    if (!runtime || operationPending.current) return
    operationPending.current = true
    setCompletion(null)
    setCopyStatus(null)
    setBusy("test")
    try {
      const tested = await testCliInstallation()
      if (statusKey(cliStore.getSnapshot().state, cliStore.getSnapshot().error) === key) {
        setTestResult({ key, sha256: tested.sha256, error: null })
      }
    } catch (error) {
      if (statusKey(cliStore.getSnapshot().state, cliStore.getSnapshot().error) === key) {
        setTestResult({ key, sha256: runtime.sha256, error: String(error) })
      }
    } finally {
      operationPending.current = false
      setBusy(null)
    }
  }

  async function repair() {
    if (operationPending.current) return
    operationPending.current = true
    setDetailsOpen(true)
    setBusy("repair")
    setTestResult(null)
    setCompletion(null)
    setCopyStatus(null)
    try {
      const repaired = await repairCliInstallation()
      cliStore.set({ state: repaired, error: null })
      if (repaired.runtime && !repaired.diagnostic) {
        setCompletion({
          key: statusKey(repaired, null),
          message: "Repair complete. Run Test to verify.",
        })
      }
    } catch (error) {
      cliStore.set({ ...cliStore.getSnapshot(), error: String(error) })
    } finally {
      operationPending.current = false
      setBusy(null)
    }
  }

  async function copyPath(path: string) {
    if (operationPending.current) return
    operationPending.current = true
    clearTimeout(copyTimer.current)
    setBusy("copy")
    setCopyStatus(null)
    setCompletion(null)
    try {
      await writeClipboardText(path)
      if (statusKey(cliStore.getSnapshot().state, cliStore.getSnapshot().error) === key) {
        if (copyLive.current) {
          setCopyStatus({ key, message: "Copied!" })
          copyTimer.current = setTimeout(() => setCopyStatus(null), 2_000)
        }
      }
    } catch {
      if (statusKey(cliStore.getSnapshot().state, cliStore.getSnapshot().error) === key) {
        setCopyStatus({ key, message: "Couldn’t copy the path. Select it to copy manually." })
      }
    } finally {
      operationPending.current = false
      setBusy(null)
    }
  }

  return (
    <Pane
      title="CLI"
      description="Helps antiburn share check findings with your coding agents."
    >
      <Card>
        <SettingsRow
          searchId="cliCommand"
          label={
            <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="inline-flex items-baseline gap-2">
                <span>Antiburn CLI</span>
                {state?.runtime && (
                  <span className="type-footnote text-label-secondary">
                    v{state.runtime.version}
                  </span>
                )}
              </span>
              {inlineStatus && (
                <span
                  className="inline-flex items-center rounded-control border border-system-green/25 bg-system-green/10 px-2 type-caption font-medium! text-system-green"
                  role="status"
                >
                  Installed
                </span>
              )}
            </span>
          }
          description="Managed automatically by antiburn."
        >
          {(needsRepair || !state) && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <p role="status" className={`type-body flex items-center gap-2 ${statusTone}`}>
                <StatusIcon size={14} className="shrink-0" aria-hidden="true" />
                {result && !result.error && state?.diagnostic
                  ? "Installation needs attention. Open technical details for troubleshooting."
                  : status}
              </p>
              {needsRepair && !detailsOpen && (
                <PushButton
                  className="min-w-28"
                  disabled={busy !== null}
                  onClick={() => {
                    technicalDetails.current
                      ?.querySelector<HTMLButtonElement>("button")
                      ?.focus()
                    void repair()
                  }}
                >
                  <Wrench size={12} aria-hidden="true" /> Repair
                </PushButton>
              )}
            </div>
          )}
          <SettingsFocusTarget ref={technicalDetails}>
            <Disclosure
              label="Technical details"
              open={detailsOpen}
              onOpenChange={setDetailsOpen}
              className="mt-1 [&>button]:px-0 [&>div]:px-0"
            >
              <div className="type-footnote space-y-4">
                <div className="space-y-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <PushButton
                      className="min-w-28"
                      disabled={busy !== null || !state?.runtime}
                      onClick={() => void test()}
                    >
                      {busy === "test" ? (
                        <LoaderCircle
                          size={12}
                          aria-hidden="true"
                          className="motion-safe:animate-spin"
                        />
                      ) : (
                        <Stethoscope size={12} aria-hidden="true" />
                      )}
                      {busy === "test" ? "Testing…" : "Test"}
                    </PushButton>
                    <PushButton
                      className="min-w-28"
                      disabled={busy !== null}
                      onClick={() => void repair()}
                    >
                      {busy === "repair" ? (
                        <LoaderCircle
                          size={12}
                          aria-hidden="true"
                          className="motion-safe:animate-spin"
                        />
                      ) : (
                        <Wrench size={12} aria-hidden="true" />
                      )}
                      {busy === "repair" ? "Repairing…" : "Repair"}
                    </PushButton>
                    {(busy === "test" || busy === "repair" || result) && (
                      <p
                        role="status"
                        className={
                          busy
                            ? "text-label-secondary"
                            : result?.error
                              ? "text-system-orange"
                              : "text-system-green"
                        }
                      >
                        {busy
                          ? status
                          : result?.error
                            ? "Couldn’t start the CLI. Try Repair."
                            : "Test passed"}
                      </p>
                    )}
                  </div>
                  {!busy && completion?.key === key && (
                    <p role="status">{completion.message}</p>
                  )}
                  {result && !result.error && <p>Tests CLI startup only.</p>}
                </div>
                {state?.runtime && (
                  <div className="space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="text-label">CLI location</p>
                      <div className="flex items-center gap-1">
                        <button
                          ref={bindCopyLifetime}
                          type="button"
                          className="relative inline-flex size-8 shrink-0 items-center justify-center rounded-control text-label-secondary hover:bg-surface-hover hover:text-label after:absolute after:-inset-1 disabled:opacity-50"
                          aria-label={
                            busy === "copy" ? "Copying path…" : copied ? "Copied!" : "Copy path"
                          }
                          title={copied ? "Copied!" : "Copy path"}
                          disabled={busy !== null}
                          onClick={() => {
                            if (state.runtime) void copyPath(state.runtime.executable)
                          }}
                        >
                          {busy === "copy" ? (
                            <LoaderCircle
                              size={14}
                              aria-hidden="true"
                              className="motion-safe:animate-spin"
                            />
                          ) : copied ? (
                            <Check size={14} aria-hidden="true" />
                          ) : (
                            <Copy size={14} aria-hidden="true" />
                          )}
                        </button>
                        <span
                          role="status"
                          aria-live="polite"
                          className="min-w-14 text-label-secondary"
                        >
                          {copied ? "Copied!" : ""}
                        </span>
                      </div>
                    </div>
                    <p className="select-text break-all font-mono">
                      {state.runtime.executable}
                    </p>
                    {!busy && copyStatus?.key === key && !copied && (
                      <p role="alert">{copyStatus.message}</p>
                    )}
                  </div>
                )}
                {detail && (
                  <div className="space-y-2">
                    <p className="text-label">Diagnostic</p>
                    <p role="alert" className="select-text break-words">
                      {detail}
                    </p>
                  </div>
                )}
              </div>
            </Disclosure>
          </SettingsFocusTarget>
        </SettingsRow>
      </Card>
    </Pane>
  )
}
