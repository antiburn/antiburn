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
 * gesture at. It is deliberately specific — naming the two derived excerpts,
 * the retention horizon, and the single network exception — because a
 * local-first app's privacy page is worth nothing if it is written in the same
 * reassuring generalities as everyone else's.
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
          stores only derived analysis, and uploads nothing. Each promise below opens into the
          specifics a reader could reasonably want to check.
        </p>
        {/* Disclosures rather than Card rows: this is explanatory prose, and a
            card of five paragraph-length rows read as settings that could not
            be changed. Collapsed by default — the labels are the contract, the
            bodies are the receipts. */}
        <DisclosureGroup>
          <Disclosure label="Sources are read, never written">
            antiburn reads the session files and read-only databases your coding agents already
            keep on this machine. It never modifies them, never deletes them, and never copies a
            transcript anywhere.
          </Disclosure>
          <Disclosure label="Only derived analysis is stored">
            The local database holds identities, file locations, and numbers the analysis
            produced — counts, durations, token totals, cost estimates. It holds no transcript
            bodies: no message text, no tool arguments, no file contents. Two short excerpts are
            kept so sessions are recognizable: a session&rsquo;s title (for agents that record
            none, the opening of your first message, capped at 200 characters) and each
            skill&rsquo;s one-line description, capped at 300 characters.
          </Disclosure>
          <Disclosure label="History is kept for two weeks">
            Sessions older than 14 days are dropped from the index automatically. The
            agents&rsquo; own files are left exactly where they are — antiburn simply stops
            describing them.
          </Disclosure>
          <Disclosure label="Nothing is uploaded">
            There is no account, no server, and no usage data collected. antiburn itself opens
            one kind of connection: it asks GitHub Releases whether a newer version exists, and
            that check sends nothing about you or your sessions.
          </Disclosure>
          <Disclosure label="One setting lets your agent go online">
            Settings &rarr; Usage has a switch, off by default, that lets antiburn run your
            coding agent in the background to refresh its own usage reading. Your agent goes
            online when it does that, exactly as it does when you use it yourself; antiburn
            reads the file it writes and opens no connection of its own. With the switch off,
            plan limits are read from whatever your agent last cached here and nothing runs.
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
