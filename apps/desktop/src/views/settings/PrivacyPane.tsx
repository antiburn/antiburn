// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { confirm } from '@tauri-apps/plugin-dialog';
import { useCallback, useState } from 'react';

import { Card } from '../../components/ui/Card';
import { Disclosure, DisclosureGroup } from '../../components/ui/Disclosure';
import { Pane } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import { ToggleRow } from '../../components/ui/ToggleRow';
import { clearLocalIndex, type AppInfo } from '../../lib/ipc';
import type { AppSettingsController } from './useAppSettings';

/**
 * Privacy: what antiburn reads, what it keeps, what leaves the machine, and how
 * to make it forget.
 *
 * This pane is the long form of a promise the rest of the app only has room to
 * gesture at. It is deliberately specific — naming what can be stored, the
 * absence of automatic expiry, and exactly what goes online and why — because
 * a local-first app's privacy page is worth nothing if it is written in the
 * same reassuring generalities as everyone else's. antiburn goes online as
 * the reader's own agent, with the reader's own credentials. What it never
 * does is need a service of ours, or hand what it
 * finds to one.
 */

/** What the "forget everything" action is currently doing. */
type ClearState =
  | { kind: 'idle' }
  | { kind: 'clearing' }
  | { kind: 'cleared'; sessions: number }
  | { kind: 'failed' };

export type PrivacyPaneProps = AppSettingsController & { info: AppInfo | null };

export function PrivacyPane({ settings, update, loaded, info }: PrivacyPaneProps) {
  const [clearState, setClearState] = useState<ClearState>({ kind: 'idle' });
  // Derived from the running build, never from a compile-time guess: a build
  // with no injected endpoint cannot send anything, and this pane's whole job
  // is to not overstate what the application does.
  const usageAnalyticsSupported = info?.usageAnalyticsSupported ?? false;
  const operator = info?.usageAnalyticsOperator ?? null;

  /**
   * Clearing the index is confirmed first, and the confirmation says the two
   * things a reader could reasonably fear: that their agents' own transcripts
   * are safe, and that antiburn will find all of this again.
   */
  const handleClear = useCallback(async () => {
    const proceed = await confirm(
      'This removes every session, analysis, and scan record antiburn has stored on this machine. Your agents’ own transcript files are not touched, and antiburn will rediscover them the next time it scans.',
      { title: 'Clear the local index?', kind: 'warning', okLabel: 'Clear index' },
    );
    if (!proceed) return;

    setClearState({ kind: 'clearing' });
    try {
      const sessions = await clearLocalIndex();
      setClearState({ kind: 'cleared', sessions });
    } catch {
      setClearState({ kind: 'failed' });
    }
  }, []);

  return (
    <Pane title="Privacy">
      <div className="space-y-3">
        <p className="type-body text-pretty text-label-secondary">
          antiburn reads the session files your coding agents already keep on this machine and
          keeps the data it needs locally. Your sessions, prompts, and file paths never leave
          it.{' '}
          {usageAnalyticsSupported
            ? 'The one thing antiburn reports about itself is anonymised usage analytics, which you can turn off below.'
            : 'This build sends no analytics at all — see below.'}{' '}
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
          <Disclosure label="History stays until you clear it">
            Indexed sessions do not expire based on age. antiburn keeps its local session data
            until you clear it; the agents&rsquo; own source files are left exactly where they
            are.
          </Disclosure>
          <Disclosure label="Your work is never uploaded">
            There is no antiburn account, and nothing of ours you have to reach for the app to
            work. Nothing derived from your sessions — no transcript, prompt, title, file path,
            repository name, token count, or cost figure — is sent anywhere, ever. antiburn does
            make requests of its own: it asks GitHub Releases whether a newer version exists;
            where a source is enabled, it can ask a provider for your current plan limits using
            the credentials your own tools already stored; and, in a released build with the
            switch below on, it sends anonymised analytics about the application itself. Handing
            a provider back a credential it issued you is not a disclosure — it already has it.
            Those analytics are the one thing that goes to us; they are listed field by field
            below, and they contain none of your work. This build
            {usageAnalyticsSupported
              ? ' can send them.'
              : ' has no analytics endpoint, so it cannot send them at all.'}
          </Disclosure>
          <Disclosure label="One setting lets antiburn go online for current figures">
            Settings &rarr; Usage has a switch, off by default, for keeping plan limits current.
            Turned on, antiburn can run your coding agent in the background to refresh its own
            usage reading, or read a provider&rsquo;s figures directly with the credentials your
            tools already have — either way, that is antiburn acting as you, online with what
            you already have access to. With the switch off, plan limits are read from whatever
            was last cached here, and nothing runs in the background to update them.
          </Disclosure>
          <Disclosure label="Exports describe real work">
            An exported session carries derived analysis plus the session&rsquo;s title and the
            paths it ran in — enough to describe what you were doing. antiburn warns before
            every export and asks where to put the file.
          </Disclosure>
        </DisclosureGroup>
      </div>

      <SectionGroup title="Anonymised analytics">
        <Card>
          <ToggleRow
            label="Send anonymised usage analytics"
            description={
              usageAnalyticsSupported
                ? `Which features get used and what breaks${
                    operator ? `, sent to ${operator}` : ''
                  }. Helps decide what to build next.`
                : 'This build has no analytics endpoint, so nothing can be sent from it. The switch is shown so the setting is visible, not because it currently does anything.'
            }
            // Gated on `loaded` as well as support. The controller starts from
            // DEFAULT_SETTINGS, where this is on, so an unguarded row would
            // paint "on" for an upgraded install whose stored answer is off —
            // and a click in that window would write the default back as
            // though the reader had chosen it.
            checked={usageAnalyticsSupported && loaded && settings.usageAnalyticsEnabled}
            onChange={(next) => void update({ usageAnalyticsEnabled: next })}
            dimmed={!usageAnalyticsSupported || !loaded}
            disabled={!usageAnalyticsSupported || !loaded}
          />
        </Card>
        {/* The list is the point. "Anonymised" is a word every application
            uses, so on its own it carries no information; what carries
            information is naming every field that goes, and naming what is
            excluded by construction rather than by policy. */}
        <Card>
          <Row
            label="Exactly what is sent"
            // Every field, including the plumbing ones. An enumeration that
            // quietly omits the dull fields is worth less than no enumeration,
            // because it reads as complete. `usage_analytics::event::Event` is
            // the source of truth; if a field is added there, it is named here
            // in the same change or the promise below stops being true.
            description="Twelve fields, and this is all of them: the word “desktop”; a random id for the message itself, so a retry is not counted twice; a random installation identifier; a random identifier for this run of the app; the event name, such as “a scan finished”; when it happened and when it was delivered; your processor architecture; a count rounded into a range, when the event has one; a short label naming which setting you changed or which kind of thing failed — the name only, never the value; the app version; and your operating system. Nothing else: the payload has no field capable of carrying anything else."
          />
          <Row
            label="What the timestamps make possible"
            // Named rather than left for a reader to work out. Two stamps plus
            // an identifier that lives 30 days is a coarse picture of when the
            // app gets used, and saying so is cheaper than being caught not
            // having said it.
            description="Because each event carries a time and the installation identifier lasts up to 30 days, these events show roughly when antiburn is used during that period. They cannot show what you were working on. If that is more than you want to share, the switch above turns all of it off."
          />
          <Row
            label="What is never sent"
            description="Sessions, transcripts, prompts, titles, file paths, repository or branch names, token counts, costs, credentials, your name, and your email address. Exact counts are not sent either: a precise number, repeated over weeks, identifies a machine on its own."
          />
          <Row
            label="Where this starts switched off"
            description="In the EU, the EEA, and the UK this begins off rather than on, because there analytics are something you opt into rather than out of. antiburn works that out from the locale and time zone your machine already reports — nothing is looked up, and neither is ever sent."
          />
          <Row
            label="The identifier is not you"
            description="It is random and is not derived from anything about your machine. It is replaced every 30 days, so events cannot be joined into a history longer than that. Turning the switch off deletes it along with anything still queued, so turning it back on later starts a new identifier that cannot be linked to the old one."
          />
          <Row
            label="The second identifier is weaker still"
            // The receiving server requires a per-run id, so the honest move
            // is to name it and say plainly how much less durable it is than
            // the installation one — not to leave a reader to notice a
            // second identifier in the list above and wonder.
            description="The identifier for a run of the app is required by the receiving server. It exists only in memory: quitting antiburn ends it, nothing on your machine remembers it afterwards, and it is replaced after 30 minutes of inactivity. It groups one run’s events together and cannot connect one run to another."
          />
          <Row
            label="What happens after it arrives"
            // Deliberately not claimed by the app. Retention and IP handling
            // belong to whoever operates the endpoint, and a promise this
            // binary cannot keep is the exact drift the deviations register
            // exists to catch — so point at the party who can make it.
            description="How long these events are kept, and whether the receiving server records the IP address that every internet request carries, are the operator’s decisions rather than the app’s. antiburn can only promise what it sends, which is the list above. The project’s docs/usage-analytics.md carries the full event catalog and the commands to check any of this yourself."
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Local data">
        <Card>
          <Row
            label="Clear the local index"
            description="Forget every session, analysis, and scan record antiburn has stored. Your agents’ transcripts are untouched, so a later scan finds them again. Your preferences, scan folders, and repository choices are kept."
            trailing={
              <PushButton
                onClick={() => void handleClear()}
                disabled={clearState.kind === 'clearing'}
              >
                {clearState.kind === 'clearing' ? 'Clearing…' : 'Clear index…'}
              </PushButton>
            }
          >
            {clearState.kind !== 'idle' && clearState.kind !== 'clearing' && (
              <div className="mt-1.5" aria-live="polite">
                {clearState.kind === 'cleared' ? (
                  <StatusText tone="secondary">
                    {clearState.sessions === 0
                      ? 'There was nothing stored to clear.'
                      : `Cleared ${clearState.sessions} ${
                          clearState.sessions === 1 ? 'session' : 'sessions'
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
    </Pane>
  );
}
