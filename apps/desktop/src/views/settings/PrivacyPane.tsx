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
import { clearLocalIndex } from '../../lib/ipc';

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

export function PrivacyPane() {
  const [clearState, setClearState] = useState<ClearState>({ kind: 'idle' });

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
          antiburn reads the session files your coding agents already keep on this machine,
          keeps the data it needs locally, and uploads nothing. Each promise below opens into
          the specifics a reader could reasonably want to check.
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
          <Disclosure label="Nothing is uploaded">
            There is no antiburn account and no antiburn server — there is nothing of ours for
            your data to reach. antiburn does make requests of its own: it asks GitHub Releases
            whether a newer version exists, and, where a source is enabled, it can ask a
            provider for your current plan limits using the credentials your own tools already
            stored. Handing a provider back a credential it issued you is not a disclosure — it
            already has it. No third party sees any of it, and no antiburn server exists to see
            it either.
          </Disclosure>
          <Disclosure label="One setting lets antiburn go online for current figures">
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
