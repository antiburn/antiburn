// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check, FolderPlus, Lock, X } from 'lucide-react';

import appIcon from '../../assets/app-icon.png';
import { useEffect, useRef, useState } from 'react';

import { FolderPermissionNotice } from '../../components/repositories/FolderPermissionNotice';
import { LocalRepositoryList } from '../../components/repositories/LocalRepositoryList';
import { PushButton } from '../../components/ui/PushButton';
import { ScrollPane } from '../../components/ui/ScrollPane';
import { SegmentedControl } from '../../components/ui/SegmentedControl';
import { openFolderAccessSettings, type ScanStatus } from '../../lib/ipc';
import type { FolderPermissions } from '../../lib/types/repository';
import type { FolderPermissionFlow } from '../../lib/useFolderPermissionFlow';
import type { LocalRepositoryItem } from '../../lib/types/repository';

/**
 * First run, in five screens: Welcome, Sources, Repositories, Historical scan,
 * Ready.
 *
 * The rhythm is the app-shell contract's — establish trust, choose where to
 * look, choose what to include, do the work with progress and a way out, then
 * confirm a usable result. What each step *does* is local: there is no account
 * to create and nothing is uploaded, so the steps carry local jobs and local
 * copy.
 *
 * ## Deliberate copy deviation
 *
 * The ratified onboarding row specifies Welcome copy that mentions "anonymous
 * analytics is sent only after a separate, default-off opt-in". This build
 * ships **no analytics of any kind** — no client, no consent surface, no
 * endpoint — so promising a control that does not exist would be the exact
 * failure the honesty rule is there to prevent. The Welcome step says what is
 * true instead, and the divergence is recorded rather than hidden. If analytics
 * are ever built, the copy and the matrix row can be reconciled then.
 */

export interface OnboardingFlowProps {
  /** Directories the engine searches without being asked. */
  defaultRoots: readonly string[];
  /**
   * Default roots the operating system is still guarding, so the step can say
   * "needs permission" rather than ticking a folder nothing has read.
   */
  blockedRoots: readonly string[];
  /** Which protected folders need permission, and which already have it. */
  permissions: FolderPermissions;
  /** The sequential request flow the notice drives. */
  permissionFlow: FolderPermissionFlow;
  /** Extra directories the reader has added so far. */
  scanRoots: readonly string[];
  /** Open a directory picker and add the result. */
  onAddScanRoot: () => void;
  onRemoveScanRoot: (path: string) => void;
  /** Repositories the first discovery pass found. */
  repositories: readonly LocalRepositoryItem[];
  /** Include or ignore one repository. */
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void;
  /** Run a discovery pass. Called when a step needs fresh results. */
  onDiscover: () => void;
  /** Stop the pass in flight. */
  onCancelScan: () => void;
  /** The shell's scan status, or null before the first read. */
  scanStatus: ScanStatus | null;
  /** How many days of history the popover will list. */
  windowDays: number;
  onWindowDaysChange: (days: number) => void;
  /** Finish: records the flag and enters the activity view. */
  onFinish: () => void;
}

const STEPS = ['welcome', 'sources', 'repositories', 'scan', 'ready'] as const;
type Step = (typeof STEPS)[number];

/** Steps that want a discovery pass to have run by the time they are read. */
const STEPS_NEEDING_DISCOVERY: readonly Step[] = ['repositories', 'scan'];

/** The two history windows the flow offers. Both fit inside the two weeks the
 *  store retains, so neither is a promise the app cannot keep. */
const WINDOW_OPTIONS = [
  { value: '7', label: '7 days' },
  { value: '14', label: '14 days' },
] as const;

function StepDots({ step }: { step: Step }) {
  const index = STEPS.indexOf(step);
  return (
    <div className="flex items-center justify-center gap-1.5" aria-hidden="true">
      {STEPS.map((name, position) => (
        <span
          key={name}
          className={`h-1.5 w-1.5 rounded-full transition-colors ${
            position === index ? 'bg-label-secondary' : 'bg-label/20'
          }`}
        />
      ))}
    </div>
  );
}

function Welcome() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <img
        src={appIcon}
        alt=""
        aria-hidden="true"
        width={96}
        height={96}
        className="mb-5 h-24 w-24 select-none drop-shadow-md"
        draggable={false}
      />
      <h2 data-step-heading tabIndex={-1} className="type-title-3 text-label outline-none">
        Everything stays on this machine
      </h2>
      <p className="mt-2 text-balance type-callout text-label-secondary">
        antiburn reads the coding-agent sessions already on your disk, analyzes them here, and
        shows you what they cost and how they went. No account, nothing uploaded, and no usage
        data collected.
      </p>
      <p className="mt-2 text-balance type-footnote text-label-tertiary">
        The only time antiburn uses the network is when it checks GitHub Releases for a new
        version.
      </p>
    </div>
  );
}

function Sources({
  defaultRoots,
  blockedRoots,
  scanRoots,
  onAddScanRoot,
  onRemoveScanRoot,
}: Pick<
  OnboardingFlowProps,
  'defaultRoots' | 'scanRoots' | 'onAddScanRoot' | 'onRemoveScanRoot'
> & { blockedRoots: readonly string[] }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-5 pt-2">
      <h2 data-step-heading tabIndex={-1} className="type-title-3 text-label outline-none">
        Where to look
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Agent session stores are found automatically. Add a folder only if you keep repositories
        somewhere unusual.
      </p>

      <ScrollPane className="mt-3" viewportClassName="pr-1">
        {defaultRoots.length > 0 && (
          <>
            <p className="pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
              {blockedRoots.length > 0 ? 'Default folders' : 'Searched already'}
            </p>
            <ul className="space-y-0.5 pb-3">
              {defaultRoots.map((root) => {
                // A default root inside a folder macOS is still guarding has
                // *not* been searched. Ticking it here would be the one lie
                // this step could tell.
                const blocked = blockedRoots.includes(root);
                return (
                  <li key={root} className="flex items-center gap-1.5">
                    {blocked ? (
                      <Lock
                        size={11}
                        strokeWidth={2.5}
                        aria-hidden="true"
                        className="shrink-0 text-label-tertiary"
                      />
                    ) : (
                      <Check
                        size={11}
                        strokeWidth={2.5}
                        aria-hidden="true"
                        className="shrink-0 text-label-tertiary"
                      />
                    )}
                    <span
                      dir="rtl"
                      className="truncate text-left type-footnote text-label-secondary"
                    >
                      <bdi>{root}</bdi>
                    </span>
                    {blocked ? (
                      <span className="shrink-0 type-caption text-label-tertiary">
                        needs permission
                      </span>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          </>
        )}

        <p className="pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
          Added by you
        </p>
        {scanRoots.length === 0 ? (
          <p className="pb-2 type-footnote text-label-tertiary">Nothing extra yet.</p>
        ) : (
          <ul className="space-y-0.5 pb-2">
            {scanRoots.map((root) => (
              <li key={root} className="flex items-center gap-1.5">
                <span
                  dir="rtl"
                  className="min-w-0 flex-1 truncate text-left type-footnote text-label-secondary"
                >
                  <bdi>{root}</bdi>
                </span>
                <button
                  type="button"
                  onClick={() => onRemoveScanRoot(root)}
                  aria-label={`Stop scanning ${root}`}
                  className="shrink-0 rounded p-0.5 text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
                >
                  <X size={11} strokeWidth={2.5} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </ScrollPane>

      <div className="pt-1 pb-1">
        <PushButton className="gap-1.5" onClick={onAddScanRoot}>
          <FolderPlus size={12} aria-hidden="true" />
          Add a folder…
        </PushButton>
      </div>
    </div>
  );
}

/**
 * Which repositories antiburn watches.
 *
 * Inclusion is opt-out, so this step is genuinely skippable — the list is here
 * because the reader may have a client's repository on this machine they would
 * rather antiburn never indexed, and the first run is the moment to say so.
 */
function Repositories({
  repositories,
  onToggleRepository,
  scanning,
  permissions,
  permissionFlow,
}: {
  repositories: readonly LocalRepositoryItem[];
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void;
  scanning: boolean;
  permissions: FolderPermissions;
  permissionFlow: FolderPermissionFlow;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-5 pt-2">
      <h2 data-step-heading tabIndex={-1} className="type-title-3 text-label outline-none">
        What to include
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Everything found here is watched unless you turn it off. Turning one off also stops its
        sessions being indexed.
      </p>
      {permissions.supported && permissions.deferred.length > 0 ? (
        <div className="mt-2">
          <FolderPermissionNotice
            deferred={permissions.deferred}
            phase={permissionFlow.phase}
            current={permissionFlow.current}
            position={permissionFlow.position}
            total={permissionFlow.total}
            recordedDenials={permissionFlow.recordedDenials}
            onRequest={permissionFlow.start}
            onOpenSettings={() => void openFolderAccessSettings()}
          />
        </div>
      ) : null}
      <div className="mt-2 min-h-0 flex-1">
        <LocalRepositoryList
          repositories={[...repositories]}
          loading={scanning}
          onToggleRepository={onToggleRepository}
          emptyTitle="Nothing found yet"
          emptyDescription="Repositories appear once a coding session has run in one. You can change this later in Settings."
        />
      </div>
    </div>
  );
}

/**
 * The historical scan: choose how much history to keep on screen, watch the
 * pass run, and stop it if it is taking too long.
 *
 * The window choice is deliberately described as what it *is* — how far back
 * the list reaches — rather than as how far back the scan goes. antiburn keeps
 * two weeks of history whichever option is picked, and saying otherwise would
 * be a nicer sentence about a thing that is not true.
 */
function HistoricalScan({
  scanStatus,
  windowDays,
  onWindowDaysChange,
  onDiscover,
  onCancelScan,
}: Pick<
  OnboardingFlowProps,
  'scanStatus' | 'windowDays' | 'onWindowDaysChange' | 'onDiscover' | 'onCancelScan'
>) {
  const running = scanStatus?.running ?? false;
  const found = scanStatus?.sessions ?? 0;

  return (
    <div className="flex min-h-0 flex-1 flex-col px-5 pt-2">
      <h2 data-step-heading tabIndex={-1} className="type-title-3 text-label outline-none">
        Historical scan
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        antiburn keeps two weeks of local history. Choose how much of it the popover lists — you
        can change this later.
      </p>

      <SegmentedControl
        className="mt-3 w-full"
        options={WINDOW_OPTIONS}
        value={String(windowDays) === '14' ? '14' : '7'}
        onChange={(days) => onWindowDaysChange(Number(days))}
        ariaLabel="How much history to list"
        equalWidth
      />

      <div className="mt-4 flex-1" aria-live="polite">
        {running ? (
          <>
            <p className="type-callout text-label">
              Scanning… {scanStatus?.completedAgents ?? 0} of {scanStatus?.totalAgents ?? 0}{' '}
              agents
            </p>
            <p className="mt-1 type-footnote text-label-secondary">
              {found} {found === 1 ? 'session' : 'sessions'} so far. Session files are read,
              never written.
            </p>
          </>
        ) : scanStatus?.error ? (
          <>
            <p className="type-callout text-system-orange">The scan did not finish.</p>
            <p className="mt-1 type-footnote text-label-secondary">{scanStatus.error}</p>
          </>
        ) : scanStatus?.cancelled ? (
          <>
            <p className="type-callout text-label">Stopped.</p>
            <p className="mt-1 type-footnote text-label-secondary">
              {found} {found === 1 ? 'session' : 'sessions'} were indexed before it stopped. You
              can continue and scan again later.
            </p>
          </>
        ) : scanStatus?.finishedAt ? (
          <>
            <p className="type-callout text-label">
              Found {found} {found === 1 ? 'session' : 'sessions'}.
            </p>
            <p className="mt-1 type-footnote text-label-secondary">
              antiburn keeps looking in the background while you use it.
            </p>
          </>
        ) : (
          <p className="type-footnote text-label-secondary">Ready to scan.</p>
        )}
      </div>

      <div className="pb-1">
        {running ? (
          <PushButton onClick={onCancelScan}>Stop</PushButton>
        ) : scanStatus?.finishedAt ? null : (
          <PushButton onClick={onDiscover}>Scan now</PushButton>
        )}
      </div>
    </div>
  );
}

function Ready({ sessions }: { sessions: number }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-surface-secondary text-label-secondary">
        <Check size={22} strokeWidth={2} aria-hidden="true" />
      </div>
      <h2 data-step-heading tabIndex={-1} className="type-title-3 text-label outline-none">
        Ready
      </h2>
      <p className="mt-2 text-balance type-callout text-label-secondary">
        {sessions > 0
          ? `${sessions} ${sessions === 1 ? 'session is' : 'sessions are'} indexed and waiting in the menu bar.`
          : 'Nothing is indexed yet — antiburn keeps looking in the background as you work.'}
      </p>
      <p className="mt-2 text-balance type-footnote text-label-tertiary">
        Session files are read only; your repositories are never modified.
      </p>
    </div>
  );
}

/** The first-run flow. See the module comment for its rhythm and its one
 *  deliberate deviation from the ratified copy. */
export function OnboardingFlow({
  defaultRoots,
  blockedRoots,
  permissions,
  permissionFlow,
  scanRoots,
  onAddScanRoot,
  onRemoveScanRoot,
  repositories,
  onToggleRepository,
  onDiscover,
  onCancelScan,
  scanStatus,
  windowDays,
  onWindowDaysChange,
  onFinish,
}: OnboardingFlowProps) {
  const [step, setStep] = useState<Step>('welcome');
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const index = STEPS.indexOf(step);
  const last = index === STEPS.length - 1;

  // A step change swaps the whole body, so focus has to be moved deliberately;
  // left alone it falls back to <body> and a screen-reader user is told
  // nothing at all about where they now are.
  useEffect(() => {
    bodyRef.current?.querySelector<HTMLElement>('[data-step-heading]')?.focus();
  }, [step]);

  // Reaching the repository or scan step with nothing discovered yet means the
  // step has nothing to show. Ask for a pass rather than presenting an empty
  // list as an answer.
  //
  // The ref is what keeps that to *one* request: `onDiscover` is a fresh arrow
  // from the host on every render, and the status it will eventually change is
  // not back yet, so a plain dependency list re-fires until it lands.
  const requested = useRef(false);
  const needsDiscovery = STEPS_NEEDING_DISCOVERY.includes(step);
  const discovered = scanStatus?.finishedAt != null;
  const running = scanStatus?.running ?? false;
  useEffect(() => {
    if (!needsDiscovery || discovered || running || requested.current) return;
    requested.current = true;
    onDiscover();
  }, [needsDiscovery, discovered, running, onDiscover]);

  return (
    <div className="flex h-full flex-col" aria-label="Set up antiburn" role="region">
      <header className="flex h-11 shrink-0 items-center px-4">
        <h1 className="type-headline text-label">antiburn</h1>
        {/* The step is announced as text rather than left to the dots, which
            are decorative and hidden. */}
        <p className="sr-only" aria-live="polite">
          Step {index + 1} of {STEPS.length}
        </p>
      </header>

      <div ref={bodyRef} className="flex min-h-0 flex-1 flex-col">
        {step === 'welcome' && <Welcome />}
        {step === 'sources' && (
          <Sources
            defaultRoots={defaultRoots}
            blockedRoots={blockedRoots}
            scanRoots={scanRoots}
            onAddScanRoot={onAddScanRoot}
            onRemoveScanRoot={onRemoveScanRoot}
          />
        )}
        {step === 'repositories' && (
          <Repositories
            repositories={repositories}
            onToggleRepository={onToggleRepository}
            scanning={running && repositories.length === 0}
            permissions={permissions}
            permissionFlow={permissionFlow}
          />
        )}
        {step === 'scan' && (
          <HistoricalScan
            scanStatus={scanStatus}
            windowDays={windowDays}
            onWindowDaysChange={onWindowDaysChange}
            onDiscover={onDiscover}
            onCancelScan={onCancelScan}
          />
        )}
        {step === 'ready' && <Ready sessions={scanStatus?.sessions ?? 0} />}
      </div>

      <footer className="flex shrink-0 items-center gap-2 border-t border-separator px-4 py-3">
        <div className="flex-1">
          {index > 0 && (
            <PushButton onClick={() => setStep(STEPS[index - 1] ?? 'welcome')}>Back</PushButton>
          )}
        </div>
        <StepDots step={step} />
        <div className="flex flex-1 justify-end">
          <PushButton
            variant="primary"
            onClick={() => (last ? onFinish() : setStep(STEPS[index + 1] ?? 'ready'))}
          >
            {last ? 'Start using antiburn' : 'Continue'}
          </PushButton>
        </div>
      </footer>
    </div>
  );
}
