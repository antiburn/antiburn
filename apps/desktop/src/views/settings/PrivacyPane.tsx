import { confirm, save } from "@tauri-apps/plugin-dialog"
import { useCallback, useState } from "react"

import { Card } from "../../components/ui/Card"
import { Disclosure, DisclosureGroup } from "../../components/ui/Disclosure"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { StatusText } from "../../components/ui/StatusText"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { exportDiagnostics } from "../../lib/diagnosticsIpc"
import {
  clearLocalIndex,
  openAnalyticsDocumentation,
  openPrivacyPolicy,
  type AppInfo,
} from "../../lib/ipc"
import type { AppSettingsController } from "./useAppSettings"

/**
 * Privacy: what antiburn reads, what it keeps, what leaves the machine, and how
 * to make it forget.
 *
 * This pane is the long form of a promise the rest of the app only has room to
 * gesture at. It is deliberately specific — naming what can be stored, its
 * configurable retention, and exactly what goes online and why — because
 * a local-first app's privacy page is worth nothing if it is written in the
 * same reassuring generalities as everyone else's. antiburn goes online as
 * the reader's own agent, with the reader's own credentials. What it never
 * does is need a service of ours, or hand what it
 * finds to one.
 */

/** What the "forget everything" action is currently doing. */
type ClearState =
  | { kind: "idle" }
  | { kind: "clearing" }
  | { kind: "cleared"; sessions: number }
  | { kind: "failed" }

/** What the diagnostics export action is currently doing. */
type DiagnosticsExportState = "idle" | "exporting" | "exported" | "failed"

type RetentionValue = "30" | "90" | "-1"

const RETENTION_OPTIONS: ReadonlyArray<{ value: RetentionValue; label: string }> = [
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "-1", label: "Forever" },
]

function retentionLength(days: number): number {
  return days === -1 ? Number.POSITIVE_INFINITY : days
}

export type PrivacyPaneProps = AppSettingsController & { info: AppInfo | null }

export function PrivacyPane({ settings, update, loaded, info }: PrivacyPaneProps) {
  const [clearState, setClearState] = useState<ClearState>({ kind: "idle" })
  const [diagnosticsExportState, setDiagnosticsExportState] =
    useState<DiagnosticsExportState>("idle")
  // Derived from the running build, never from a compile-time guess: a build
  // with no injected endpoint cannot send anything, and this pane's whole job
  // is to not overstate what the application does.
  const analyticsSupported = info?.analyticsSupported ?? false
  const analyticsEnvironmentDisabled = info?.analyticsEnvironmentDisabled ?? false
  const operator = info?.analyticsOperator ?? null

  async function handleRetentionChange(value: RetentionValue) {
    const days = Number(value)
    if (retentionLength(days) < retentionLength(settings.sessionDataRetentionDays)) {
      const period = days === 30 ? "30 days" : "90 days"
      const proceed = await confirm(
        `This immediately removes antiburn’s local data for sessions whose last activity is older than ${period}. Providers retain session history for only 30 days, so antiburn may hold the only remaining history. Your coding agents’ transcript files are not touched.`,
        {
          title: `Keep session data for ${period}?`,
          kind: "warning",
          okLabel: "Change retention",
        },
      )
      if (!proceed) return
    }
    await update({ sessionDataRetentionDays: days })
  }

  /**
   * Clearing the index is confirmed first, and the confirmation says the two
   * things a reader could reasonably fear: that their agents' own transcripts
   * are safe, and that antiburn will find all of this again.
   */
  const handleClear = useCallback(async () => {
    const proceed = await confirm(
      "This removes every session, analysis, evidence, and scan record antiburn has stored on this machine. Your agents’ own transcript files are not touched, and antiburn will rediscover them the next time it scans.",
      { title: "Clear the local index?", kind: "warning", okLabel: "Clear index" },
    )
    if (!proceed) return

    setClearState({ kind: "clearing" })
    try {
      const sessions = await clearLocalIndex()
      setClearState({ kind: "cleared", sessions })
    } catch {
      setClearState({ kind: "failed" })
    }
  }, [])

  async function handleDiagnosticsExport() {
    const proceed = await confirm(
      "The export covers up to 500 recent indexed sessions. It excludes transcript bodies, titles, file paths, working directories, repository names, account identifiers, and analytics identifiers. It does include opaque session ids and derived model, tool, skill, timing, evidence, and error data. Review it before sharing.",
      {
        title: "Export diagnostics?",
        kind: "warning",
        okLabel: "Choose destination…",
      },
    )
    if (!proceed) return

    const destination = await save({
      defaultPath: "antiburn-diagnostics.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    })
    if (!destination) return

    setDiagnosticsExportState("exporting")
    try {
      await exportDiagnostics(destination)
      setDiagnosticsExportState("exported")
    } catch {
      setDiagnosticsExportState("failed")
    }
  }

  return (
    <Pane title="Privacy">
      <div className="space-y-3">
        {/* `px-1` to match `SectionGroup`'s header and `Disclosure`'s label,
            both of which sit at the same inset. Cards indent their own
            contents to 16px; bare prose belongs on the section's line, not
            4px to the left of everything it introduces. */}
        <p className="type-body px-1 text-pretty text-label-secondary">
          antiburn reads the session files your coding agents already keep on this machine and
          keeps the data it needs locally. Your sessions, prompts, and file paths never leave
          it.{" "}
          {analyticsSupported
            ? "The one thing antiburn reports about itself is anonymised analytics, which you can turn off below."
            : "This build sends no analytics at all."}{" "}
          Each promise opens into the specifics a reader could reasonably want to check.
        </p>
        {/* Disclosures rather than Card rows: this is explanatory prose, and a
            card of five paragraph-length rows read as settings that could not
            be changed. Collapsed by default — the labels are the contract, the
            bodies are the receipts. */}
        <DisclosureGroup>
          <Disclosure label="Sources are read, never written">
            antiburn reads the session files and read-only databases your coding agents already
            keep on this machine. It may copy data into its own local store, but it never
            modifies or deletes the source transcripts.
          </Disclosure>
          <Disclosure label="Visibility data stays on this machine">
            antiburn may keep session content and derived analysis in its own local store when
            they are needed for visibility or analysis. That can include messages, tool
            activity, file content recorded in a transcript, identities, paths, counts,
            durations, token totals, and cost estimates. Nothing in this store is uploaded.
          </Disclosure>
          <Disclosure label="You control how long history stays">
            antiburn keeps indexed session data for the period you select below. The default is
            forever, which can preserve history after a provider&rsquo;s 30-day retention
            window. Shorter periods keep the local index lighter. The agents&rsquo; own source
            files are left exactly where they are.
          </Disclosure>
          <Disclosure label="Your work is never uploaded">
            There is no antiburn account, and nothing of ours you have to reach for the app to
            work. Nothing derived from your sessions — no transcript, prompt, title, file path,
            repository name, token count, or cost figure — is sent anywhere, ever. antiburn does
            make requests of its own: it downloads public model prices from models.dev at
            startup and hourly while running; it asks GitHub Releases whether a newer version
            exists; where a source is enabled, it can ask a provider for your current plan
            limits using the credentials your own tools already stored; and, in a released build
            with the switch below on, it sends anonymised analytics about the application
            itself. Handing a provider back a credential it issued you is not a disclosure — it
            already has it. Those analytics are the one thing that goes to us; they are listed
            field by field below, and they contain none of your work. This build
            {analyticsSupported
              ? " can send them."
              : " has no analytics endpoint, so it cannot send them at all."}
          </Disclosure>
          <Disclosure label="Provider plan-limit access is optional">
            Settings &rarr; Usage has a switch, on by default once first-run setup is complete,
            for keeping plan limits current. On, antiburn asks each provider directly for your
            current usage, using the credentials your own coding tools already have — that is
            antiburn acting as you, online with what you already have access to, and it runs
            without asking first because it is your own ordinary traffic, not something that
            needs a separate go-ahead. When a provider cannot be reached directly, antiburn
            falls back to asking your coding tool&rsquo;s own local process the same question,
            over its own protocol. Turn the switch off if you want none of it — no request is
            made, no credential is read, and antiburn shows no plan limits at all.
          </Disclosure>
          <Disclosure label="Exports describe real work">
            An exported session carries derived analysis plus the session&rsquo;s title and the
            paths it ran in — enough to describe what you were doing. antiburn warns before
            every export and asks where to put the file.
          </Disclosure>
        </DisclosureGroup>
      </div>

      {analyticsSupported ? (
        <SectionGroup title="Analytics">
          <Card>
            <ToggleRow
              label="Share product analytics"
              description={
                analyticsEnvironmentDisabled
                  ? "Off for this launch because ANTIBURN_ANALYTICS_ENABLED=false. Remove it to use this setting."
                  : loaded && !settings.analyticsEnabled
                    ? "Off. Antiburn deleted its analytics identifier and anything waiting to be sent."
                    : `Sends app launches, onboarding progress, feature use, and error categories${
                        operator ? ` to ${operator}` : ""
                      }. Never prompts, sessions, source code, filenames, or paths.`
              }
              // Gated on `loaded` as well as support. The controller starts from
              // DEFAULT_SETTINGS, where this is on, so an unguarded row would
              // paint "on" for an upgraded install whose stored answer is off —
              // and a click in that window would write the default back as
              // though the reader had chosen it.
              checked={!analyticsEnvironmentDisabled && loaded && settings.analyticsEnabled}
              onChange={(next) => void update({ analyticsEnabled: next })}
              dimmed={!loaded}
              disabled={analyticsEnvironmentDisabled || !loaded}
            />
          </Card>
          {/* The switch's terms sit below the card rather than inside it, which
            is the same shape the Privacy group above uses. A `Disclosure`
            owns its own hairline and its own 4px inset; nesting the group in
            a `Card` lands a second border directly on the card's own. */}
          <DisclosureGroup>
            <Disclosure label="Exactly what is sent">
              {/* Every field, including the plumbing ones. An enumeration that
                  quietly omits the dull fields is worth less than no
                  enumeration, because it reads as complete.
                  `analytics::event::Event` is the source of truth; if a
                  field is added there, it is named here in the same change or
                  the promise below stops being true.

                  A list rather than one long sentence: a reader auditing this
                  is counting items against that struct, and thirteen clauses
                  separated by semicolons cannot be counted. */}
              <p>Thirteen fields, and these are all of them:</p>
              {/* `pl-7`, not the `pl-4` this started as. Root font-size here
                  is 13px, so `pl-4` is 13px of padding — less than the disc
                  marker's own 17.5px advance, which left the bullets painting
                  4.5px *left* of the paragraph above and reading as an
                  unindented list. 22.75px clears the marker and indents it. */}
              <ul className="mt-2 list-disc space-y-0.5 pl-7">
                <li>The word &ldquo;desktop&rdquo;.</li>
                <li>A random id for the message, so a retry is not counted twice.</li>
                <li>A random installation id.</li>
                <li>A random id for this run of the app.</li>
                <li>The event name, such as &ldquo;a scan finished&rdquo;.</li>
                <li>When it happened.</li>
                <li>When it was delivered.</li>
                <li>Your processor architecture.</li>
                <li>A count rounded to a range, when the event has one.</li>
                <li>
                  A short label &mdash; which setting you changed, which agent recorded a
                  session, what kind of thing failed. The name only, never the value.
                </li>
                <li>
                  A second such label when an event has two things to tell apart, such as native
                  versus WSL.
                </li>
                <li>The app version.</li>
                <li>Your operating system.</li>
              </ul>
              {/* The complement belongs here rather than in a row of its own.
                  A reader checking the list against their own worry is asking
                  one question, and answering it two accordions apart made them
                  open both to find out. */}
              <p className="mt-2">
                Never your sessions, transcripts, prompts, titles, file paths, repository or
                branch names, token counts, costs, credentials, name, or email address. Not even
                exact counts: a precise number, repeated week after week, identifies a machine
                on its own.
              </p>
            </Disclosure>
            {/* Both identifiers in one place, with what the timestamps add.
                They were three rows, but the honest claim only holds when all
                three facts are read together: a 30-day id plus a time on every
                event is a coarse picture of when the app gets used, and saying
                so is cheaper than being caught not having said it. */}
            <Disclosure label="The two identifiers">
              <p>
                Both are random. Neither is derived from anything about you or your machine.
              </p>
              <p className="mt-2">
                The <strong className="font-medium text-label">installation id</strong> is
                replaced every 30 days, so events cannot be joined into a history longer than
                that. Since every event also carries a time, they do show roughly when antiburn
                is used within those 30 days &mdash; never what you were working on. Switching
                analytics off deletes the id and anything still queued, so switching back on
                starts a new id that cannot be linked to the old one.
              </p>
              <p className="mt-2">
                The <strong className="font-medium text-label">run id</strong> is required by
                the receiving server. It exists only in memory: quitting antiburn ends it,
                nothing on your machine remembers it, and it is replaced after 30 minutes of
                inactivity. It groups one run&rsquo;s events and cannot connect one run to
                another.
              </p>
            </Disclosure>
            <Disclosure label="How the starting default works">
              <p>
                Official release builds start with analytics on. Source builds do not include
                the analytics client unless the builder selects its Cargo feature and configures
                an endpoint and operator name.
              </p>
              <p className="mt-2">
                Set <code>ANTIBURN_ANALYTICS_ENABLED=false</code> in the app&rsquo;s launch
                environment for a process-level opt-out that takes priority over this switch.
              </p>
            </Disclosure>
            {/* Deliberately not claimed by the app. Retention and IP handling
                belong to whoever operates the endpoint, and a promise this
                binary cannot keep is the exact drift the deviations register
                exists to catch — so point at the party who can make it. */}
            <Disclosure label="What happens after it arrives">
              The first-party endpoint stores the request IP address and user-agent with the raw
              event. Raw events are retained until the operator deletes them. The privacy policy
              explains this processing in full.
            </Disclosure>
          </DisclosureGroup>
          <div className="flex gap-2 px-1">
            <PushButton onClick={() => void openAnalyticsDocumentation()}>
              Analytics documentation
            </PushButton>
            <PushButton onClick={() => void openPrivacyPolicy()}>Privacy policy</PushButton>
          </div>
        </SectionGroup>
      ) : null}

      <SectionGroup title="Local data">
        <Card>
          <Row
            label="Keep session data"
            description="All session data stays on this machine. Keeping it longer preserves history after providers’ 30-day retention window; a shorter period keeps antiburn’s local index lighter."
            trailing={
              <SegmentedControl
                options={RETENTION_OPTIONS}
                value={String(settings.sessionDataRetentionDays) as RetentionValue}
                ariaLabel="Session data retention"
                onChange={(value) => void handleRetentionChange(value)}
                disabled={!loaded}
              />
            }
          />
          <Row
            label="Clear the local index"
            description="Forget every session, analysis, evidence, and scan record antiburn has stored. Your agents’ transcripts are untouched, so a later scan finds them again. Your preferences, scan folders, and repository choices are kept."
            trailing={
              <PushButton
                onClick={() => void handleClear()}
                disabled={clearState.kind === "clearing"}
              >
                {clearState.kind === "clearing" ? "Clearing…" : "Clear index…"}
              </PushButton>
            }
          >
            {clearState.kind !== "idle" && clearState.kind !== "clearing" && (
              <div className="mt-1.5" aria-live="polite">
                {clearState.kind === "cleared" ? (
                  <StatusText tone="secondary">
                    {clearState.sessions === 0
                      ? "There was nothing stored to clear."
                      : `Cleared ${clearState.sessions} ${
                          clearState.sessions === 1 ? "session" : "sessions"
                        }. A scan is running to find them again.`}
                  </StatusText>
                ) : (
                  <StatusText tone="secondary">The index could not be cleared.</StatusText>
                )}
              </div>
            )}
          </Row>
          <Row
            label="Delete a coding agent’s own files"
            // Stated as a non-feature on purpose: "delete" in an app that reads
            // someone else's files is exactly the thing a reader should be able
            // to check, and finding nothing is not an answer.
            description="antiburn cannot do this, by design. Removing a conversation is your agent’s job, in your agent’s own interface. antiburn only ever deletes records it created itself."
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Diagnostics">
        <Card>
          <Row
            label="Export diagnostics"
            description="Create a JSON file for up to 500 recent indexed sessions, with derived evidence, processing state, revisions, errors, and aggregate turn signals. It excludes transcript bodies, titles, paths, working directories, repositories, account identifiers, analytics identifiers, and turn content. Model, tool, and skill names and descriptions can still describe real work, so review the file before sharing it."
            trailing={
              <PushButton
                onClick={() => void handleDiagnosticsExport()}
                disabled={diagnosticsExportState === "exporting"}
              >
                {diagnosticsExportState === "exporting" ? "Exporting…" : "Export…"}
              </PushButton>
            }
          >
            {diagnosticsExportState === "exported" && (
              <div className="mt-1.5" aria-live="polite">
                <StatusText tone="secondary">Diagnostics exported.</StatusText>
              </div>
            )}
            {diagnosticsExportState === "failed" && (
              <div className="mt-1.5" aria-live="polite">
                <StatusText tone="secondary">The diagnostics could not be exported.</StatusText>
              </div>
            )}
          </Row>
        </Card>
      </SectionGroup>
    </Pane>
  )
}
