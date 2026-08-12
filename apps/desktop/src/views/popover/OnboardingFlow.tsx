// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check, FolderPlus, HardDrive, X } from 'lucide-react';
import { useState } from 'react';

import { PushButton } from '../../components/ui/PushButton';
import { ScrollPane } from '../../components/ui/ScrollPane';

/**
 * First run, in three screens.
 *
 * Short on purpose. antiburn works on what is already on the machine, so the
 * only genuine decision is whether to look anywhere beyond the places agents
 * and developers already keep things — and even that has a sane answer
 * ("nowhere extra"). The flow therefore explains what the app does, offers the
 * one decision, and gets out of the way.
 */

export interface OnboardingFlowProps {
  /** Directories the engine searches without being asked. */
  defaultRoots: readonly string[];
  /** Extra directories the reader has added so far. */
  scanRoots: readonly string[];
  /** Open a directory picker and add the result. */
  onAddScanRoot: () => void;
  onRemoveScanRoot: (path: string) => void;
  /** Finish: records the flag and starts the first scan. */
  onFinish: () => void;
}

const STEPS = ['welcome', 'sources', 'ready'] as const;
type Step = (typeof STEPS)[number];

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
      <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-surface-secondary text-label-secondary">
        <HardDrive size={22} strokeWidth={1.75} aria-hidden="true" />
      </div>
      <h2 className="type-title-3 text-label">Everything stays on this machine</h2>
      <p className="mt-2 text-balance type-callout text-label-secondary">
        antiburn reads the coding-agent sessions already on your disk, analyzes them here, and
        shows you what they cost and how they went. Nothing is uploaded.
      </p>
    </div>
  );
}

function Sources({
  defaultRoots,
  scanRoots,
  onAddScanRoot,
  onRemoveScanRoot,
}: Pick<
  OnboardingFlowProps,
  'defaultRoots' | 'scanRoots' | 'onAddScanRoot' | 'onRemoveScanRoot'
>) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-5 pt-2">
      <h2 className="type-title-3 text-label">Where to look</h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Agent session stores are found automatically. Add a folder only if you keep repositories
        somewhere unusual.
      </p>

      <ScrollPane className="mt-3" viewportClassName="pr-1">
        {defaultRoots.length > 0 && (
          <>
            <p className="pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
              Searched already
            </p>
            <ul className="space-y-0.5 pb-3">
              {defaultRoots.map((root) => (
                <li key={root} className="flex items-center gap-1.5">
                  <Check
                    size={11}
                    strokeWidth={2.5}
                    aria-hidden="true"
                    className="shrink-0 text-label-tertiary"
                  />
                  <span
                    dir="rtl"
                    className="truncate text-left type-footnote text-label-secondary"
                  >
                    <bdi>{root}</bdi>
                  </span>
                </li>
              ))}
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

function Ready() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-surface-secondary text-label-secondary">
        <Check size={22} strokeWidth={2} aria-hidden="true" />
      </div>
      <h2 className="type-title-3 text-label">Ready</h2>
      <p className="mt-2 text-balance type-callout text-label-secondary">
        The first scan runs as soon as you finish. It reads session files only — your
        repositories are never modified.
      </p>
    </div>
  );
}

/** The first-run flow. See the module comment for why it is this short. */
export function OnboardingFlow({
  defaultRoots,
  scanRoots,
  onAddScanRoot,
  onRemoveScanRoot,
  onFinish,
}: OnboardingFlowProps) {
  const [step, setStep] = useState<Step>('welcome');
  const index = STEPS.indexOf(step);
  const last = index === STEPS.length - 1;

  return (
    <div className="flex h-full flex-col" aria-label="Set up antiburn" role="region">
      <header className="flex h-11 shrink-0 items-center px-4">
        <h1 className="type-headline text-label">antiburn</h1>
      </header>

      {step === 'welcome' && <Welcome />}
      {step === 'sources' && (
        <Sources
          defaultRoots={defaultRoots}
          scanRoots={scanRoots}
          onAddScanRoot={onAddScanRoot}
          onRemoveScanRoot={onRemoveScanRoot}
        />
      )}
      {step === 'ready' && <Ready />}

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
            {last ? 'Start scanning' : 'Continue'}
          </PushButton>
        </div>
      </footer>
    </div>
  );
}
