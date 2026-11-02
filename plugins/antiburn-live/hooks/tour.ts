import type { LiveChecks, SkillInfo, StepUsage } from './checks'

/**
 * One step of the demo tour: the events of one main turn. The driver calls `turnEnd`
 * after `run`, so each step goes through the real gates of `LiveChecks`.
 */
export type TourBeat = {
  title: string
  /** True when the driver dismisses the strip before this step, so the strip comes back with "(N new)". */
  dismissesStrip: boolean
  run: (checks: LiveChecks) => void
}

const MODEL = 'claude-opus-5'
const SMALL: StepUsage = { input_tokens: 10, cache_read_input_tokens: 20_000, cache_creation_input_tokens: 0 }
const DEEP: StepUsage = { input_tokens: 10, cache_read_input_tokens: 420_000, cache_creation_input_tokens: 0 }
const WORKFLOW = ['git status', 'git branch', 'date']
const SKILLS: readonly SkillInfo[] = [
  { name: 'release-notes', description: 'Draft release notes from the merged pull requests since the last tag' },
]

function steps(checks: LiveChecks, count: number, effort: string, usage: StepUsage = SMALL): void {
  for (let i = 0; i < count; i += 1) checks.step(undefined, MODEL, effort, usage)
}

function runWorkflow(checks: LiveChecks): void {
  for (const command of WORKFLOW) checks.toolResult(undefined, 'Bash', { command }, false, false, 200)
}

function failRetries(checks: LiveChecks): void {
  const input = { command: 'ls /antiburn-nope' }
  for (let i = 0; i < 3; i += 1) {
    checks.toolCall(undefined, 'Bash', 'Bash:command=ls /antiburn-nope', true)
    checks.toolResult(undefined, 'Bash', input, true, true, 60)
  }
}

function readFiles(checks: LiveChecks, files: number, reads: number, chars: number): void {
  for (let i = 0; i < reads; i += 1) {
    checks.toolResult(undefined, 'Read', { file_path: `/work/src/module-${i % files}.ts` }, true, false, chars)
  }
}

function edit(checks: LiveChecks, path: string): void {
  checks.toolResult(undefined, 'Edit', { file_path: path }, false, false, 100)
}

/**
 * The demo tour. Each step adds evidence as a session does: X1 is the first
 * warning, three checks arrive while the strip is dismissed, D comes with its
 * read cause, then K1 and C1. X1 resolves on step 3, by its fix when the person
 * pressed it. O fires only on turn 1, so the tour leaves it to the sample view.
 * The first step holds two turns, so T and K2 reach their three-turn gates on step 2.
 */
export const TOUR: readonly TourBeat[] = [
  {
    title: 'A retry loop is the first warning',
    dismissesStrip: false,
    run: checks => {
      // A quiet first turn starts the evidence that T and K2 build over three turns.
      checks.prompt('Add a retry to the sync client')
      steps(checks, 2, 'max')
      runWorkflow(checks)
      checks.turnEnd(undefined)
      steps(checks, 2, 'max')
      failRetries(checks)
      runWorkflow(checks)
    },
  },
  {
    title: 'Three more checks arrive',
    dismissesStrip: true,
    run: checks => {
      steps(checks, 2, 'max')
      failRetries(checks)
      readFiles(checks, 12, 22, 2_000)
      runWorkflow(checks)
    },
  },
  {
    title: 'The context gets heavy, and the retry loop stops',
    dismissesStrip: false,
    run: checks => {
      readFiles(checks, 6, 6, 150_000)
      steps(checks, 1, 'max', DEEP)
    },
  },
  {
    title: 'A skill goes unused, and edits leave the plan',
    dismissesStrip: false,
    run: checks => {
      checks.prompt('Draft the release notes from the merged pull requests since the last tag')
      checks.planApproved('Change parser.ts and lexer.ts.')
      for (const path of ['/work/src/cache.ts', '/work/src/config.ts', '/work/src/logger.ts']) edit(checks, path)
      // The context stays deep, so D stays active.
      steps(checks, 12, 'max', DEEP)
      if (checks.needsSkills()) checks.skills(SKILLS)
    },
  },
]
