import { expect, test } from 'claude-code/testing'

import { LiveChecks, canonicalModel, commandName, isReadCommand, liveChecks, matchSkill, namesSkillLate, withoutHostBlocks, type CheckId, type CheckView } from './checks'

const SMALL = { input_tokens: 10, cache_read_input_tokens: 20_000, cache_creation_input_tokens: 0 }
const DEEP = { input_tokens: 1, cache_read_input_tokens: 400_000, cache_creation_input_tokens: 0 }

function stageOf(checks: LiveChecks, id: CheckId) {
  return checks.takeView().checks.find(check => check.id === id)?.stage
}

function first(view: { checks: CheckView[] }): CheckView {
  const check = view.checks[0]
  if (check === undefined) throw new Error('expected one check in the view')
  return check
}

function failTimes(checks: LiveChecks, times: number, thread?: string) {
  for (let i = 0; i < times; i += 1) checks.toolCall(thread, 'Bash', 'Bash:npm test', true)
}

test('X1 stays silent below the repeat gate', () => {
  const checks = new LiveChecks()
  failTimes(checks, 2)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'X1')).toBe(undefined)
})

test('X1 fires at the gate, then resolves on a clean turn', () => {
  const checks = new LiveChecks()
  failTimes(checks, 3)
  expect(checks.turnEnd(undefined)).toBe(true)
  const view = checks.takeView()
  expect(view.newIds).toEqual(['X1'])
  expect(first(view).metric).toBe('3 tries · Bash')
  checks.turnEnd(undefined)
  const after = checks.takeView()
  expect(first(after).stage).toBe('resolved')
  expect(after.resolvedNow).toEqual(['X1'])
})

test('X1 does not count a call that succeeded in between', () => {
  const checks = new LiveChecks()
  failTimes(checks, 2)
  checks.toolCall(undefined, 'Bash', 'Bash:npm test', false)
  failTimes(checks, 2)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'X1')).toBe(undefined)
})

test('X1 re-fires only at the next count step', () => {
  const checks = new LiveChecks()
  failTimes(checks, 3)
  checks.turnEnd(undefined)
  checks.takeView()
  failTimes(checks, 4)
  checks.turnEnd(undefined)
  expect(checks.takeView().newIds).toEqual([])
  failTimes(checks, 5)
  checks.turnEnd(undefined)
  expect(checks.takeView().newIds).toEqual(['X1'])
})

test('X1 counts a subagent loop in its own thread', () => {
  const checks = new LiveChecks()
  failTimes(checks, 2, 'agent-1')
  failTimes(checks, 1)
  checks.turnEnd('agent-1')
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'X1')).toBe(undefined)
  failTimes(checks, 3, 'agent-2')
  checks.turnEnd('agent-2')
  checks.turnEnd(undefined)
  expect(first(checks.takeView()).metric).toBe('3 tries · Bash · subagent')
})

test('Context heavy fires on the D depth rule and compaction resolves it', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-5', 'high', DEEP)
  checks.turnEnd(undefined)
  const view = checks.takeView()
  expect(first(view).id).toBe('D')
  expect(first(view).metric).toBe('400k tokens per request')
  expect(checks.compact()).toBe(true)
  expect(stageOf(checks, 'D')).toBe('resolved')
})

test('Context heavy stays silent at the depth cap', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-5', 'high', { ...DEEP, input_tokens: 0 })
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'D')).toBe(undefined)
})

test('Context heavy fires on the X9 rule after two high readings', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-5', 'high', SMALL)
  checks.turnEnd(undefined)
  expect(checks.measure(92)).toBe(false)
  expect(checks.measure(93)).toBe(true)
  expect(first(checks.takeView()).metric).toBe('93% of context used')
})

test('Context heavy does not show in the middle of a turn', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-5', 'high', SMALL)
  checks.measure(95)
  expect(checks.measure(95)).toBe(false)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'D')).toBe('active')
})

test('subagent steps do not raise Context heavy or Overthinking', () => {
  const checks = new LiveChecks()
  for (let i = 0; i < 8; i += 1) checks.step('agent-1', 'claude-opus-5', 'max', DEEP)
  checks.turnEnd('agent-1')
  checks.turnEnd(undefined)
  expect(checks.takeView().checks).toEqual([])
})

test('O fires on the first turn with the replacement in its prompt', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-4-8-20260115', 'high', SMALL)
  checks.turnEnd(undefined)
  const check = first(checks.takeView())
  expect(check.id).toBe('O')
  expect(check.prompt).toBe('/model claude-opus-5')
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'O')).toBe('quiet')
})

test('O resolves after a switch to a current model', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-sonnet-4-6', 'high', SMALL)
  checks.turnEnd(undefined)
  checks.step(undefined, 'claude-sonnet-5', 'high', SMALL)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'O')).toBe('resolved')
})

test('an unknown model gives no O', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-haiku-4-5', 'high', SMALL)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'O')).toBe(undefined)
})

test('T builds evidence through the grace period, then fires', () => {
  const checks = new LiveChecks()
  for (let turn = 1; turn <= 2; turn += 1) {
    for (let i = 0; i < 3; i += 1) checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    checks.turnEnd(undefined)
  }
  const building = first(checks.takeView())
  expect(building.stage).toBe('building')
  expect(building.progress).toBe('turn 2 of 3')
  expect(liveChecks(checks.takeView())).toEqual([])
  checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
  checks.turnEnd(undefined)
  const active = first(checks.takeView())
  expect(active.stage).toBe('active')
  expect(active.metric).toBe('7 of 7 requests at xhigh')
})

test('T stays silent when above-cap requests are a small share', () => {
  const quiet = new LiveChecks()
  for (let turn = 1; turn <= 3; turn += 1) {
    quiet.step(undefined, 'claude-opus-5', 'low', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'low', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'low', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    quiet.step(undefined, 'claude-opus-5', 'low', SMALL)
    quiet.turnEnd(undefined)
  }
  expect(stageOf(quiet, 'T')).toBe(undefined)
})

test('T progress names the share when only the share is low', () => {
  const checks = new LiveChecks()
  for (let turn = 1; turn <= 3; turn += 1) {
    for (let i = 0; i < 3; i += 1) checks.step(undefined, 'claude-opus-5', 'low', SMALL)
    checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    checks.turnEnd(undefined)
  }
  const building = first(checks.takeView())
  expect(building.stage).toBe('building')
  expect(building.progress).toBe('40% of requests at xhigh, needs 50%')
})

test('a sent fix watches, then stops the check', () => {
  const checks = new LiveChecks()
  failTimes(checks, 3)
  checks.turnEnd(undefined)
  checks.fixSent('X1')
  expect(stageOf(checks, 'X1')).toBe('watching')
  checks.turnEnd(undefined)
  const check = first(checks.takeView())
  expect(check.stage).toBe('resolved')
  expect(check.isStoppedAfterFix).toBe(true)
})

test('a fill action watches only after a request shows the change', () => {
  const checks = new LiveChecks()
  for (let turn = 0; turn < 3; turn += 1) {
    for (let i = 0; i < 3; i += 1) checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
    checks.turnEnd(undefined)
  }
  checks.fillPressed('T')
  expect(stageOf(checks, 'T')).toBe('active')
  checks.step(undefined, 'claude-opus-5', 'xhigh', SMALL)
  expect(stageOf(checks, 'T')).toBe('active')
  checks.step(undefined, 'claude-opus-5', 'high', SMALL)
  expect(stageOf(checks, 'T')).toBe('watching')
  checks.turnEnd(undefined)
  const check = first(checks.takeView())
  expect(check.stage).toBe('resolved')
  expect(check.isStoppedAfterFix).toBe(true)
})

test('a fix that does not help returns to active after the watch', () => {
  const checks = new LiveChecks()
  checks.step(undefined, 'claude-opus-5', 'high', DEEP)
  checks.turnEnd(undefined)
  checks.fixSent('D')
  for (let turn = 0; turn < 3; turn += 1) {
    checks.step(undefined, 'claude-opus-5', 'high', DEEP)
    checks.turnEnd(undefined)
  }
  expect(stageOf(checks, 'D')).toBe('active')
})

test('a hidden check leaves the live count', () => {
  const checks = new LiveChecks()
  failTimes(checks, 3)
  checks.turnEnd(undefined)
  checks.hide('X1')
  expect(liveChecks(checks.takeView())).toEqual([])
})

test('model keys normalize like the Rust catalog', () => {
  expect(canonicalModel('anthropic/claude-opus-4.8')).toBe('claude-opus-4.8')
  expect(canonicalModel(' Claude-Opus-4-8-20260115 ')).toBe('claude-opus-4-8')
  expect(canonicalModel('antigravity-claude-opus-4-6-thinking')).toBe('claude-opus-4-6-thinking')
})

function readFiles(checks: LiveChecks, count: number, paths: number) {
  for (let i = 0; i < count; i += 1) {
    checks.toolResult(undefined, 'Read', { file_path: `/work/src/f${i % paths}.ts` }, true, false, 4_000)
  }
}

test('E1 fires on a turn of many reads over many files with no edit', () => {
  const checks = new LiveChecks()
  readFiles(checks, 24, 17)
  checks.turnEnd(undefined)
  const e1 = checks.takeView().checks.find(check => check.id === 'E1')
  expect(e1?.stage).toBe('active')
  expect(e1?.metric).toBe('24 reads · 0 edits · 17 files')
})

test('E1 stays silent below the path gate, and for a turn that edits', () => {
  const checks = new LiveChecks()
  readFiles(checks, 30, 4)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'E1')).toBe(undefined)
  readFiles(checks, 24, 17)
  checks.toolResult(undefined, 'Edit', { file_path: '/work/src/f1.ts' }, false, false, 100)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'E1')).toBe(undefined)
})

test('E1 resolves after the fix when the next turn edits a file', () => {
  const checks = new LiveChecks()
  readFiles(checks, 24, 17)
  checks.turnEnd(undefined)
  checks.fixSent('E1')
  expect(stageOf(checks, 'E1')).toBe('watching')
  checks.toolResult(undefined, 'Write', { file_path: '/work/src/new.ts' }, false, false, 100)
  checks.turnEnd(undefined)
  const e1 = checks.takeView().checks.find(check => check.id === 'E1')
  expect(e1?.stage).toBe('resolved')
  expect(e1?.isStoppedAfterFix).toBe(true)
})

test('E1 ignores subagent reads, which stay out of the main context', () => {
  const checks = new LiveChecks()
  for (let i = 0; i < 30; i += 1) checks.toolResult('agent-1', 'Read', { file_path: `/f${i}` }, true, false, 4_000)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'E1')).toBe(undefined)
})

test('E1 counts read-only Bash commands by program', () => {
  expect(isReadCommand('sed -n 1,80p src/a.ts')).toBe(true)
  expect(isReadCommand('rg -n foo src | head -20')).toBe(true)
  expect(isReadCommand('cat a.ts 2>&1')).toBe(true)
  expect(isReadCommand('sed -i "" s/a/b/ a.ts')).toBe(false)
  expect(isReadCommand('cat a.ts > b.ts')).toBe(false)
  expect(isReadCommand('npm test')).toBe(false)
  expect(isReadCommand('ls && npm run build')).toBe(false)
})

test('E2 names file reads as the cause of a heavy context, and compact keeps the findings', () => {
  const checks = new LiveChecks()
  // 300 reads of 4,000 characters are about 300k tokens of a 400k-token request.
  readFiles(checks, 300, 5)
  checks.step(undefined, 'claude-opus-5', 'high', DEEP)
  checks.turnEnd(undefined)
  const d = checks.takeView().checks.find(check => check.id === 'D')
  expect(d?.cause).toBe('File reads are about 75% of it (300k tokens).')
  expect(d?.prompt.endsWith('drop the raw file contents.')).toBe(true)
})

test('E2 gives no cause when reads are a small part of the context', () => {
  const checks = new LiveChecks()
  readFiles(checks, 10, 5)
  checks.step(undefined, 'claude-opus-5', 'high', DEEP)
  checks.turnEnd(undefined)
  expect(checks.takeView().checks.find(check => check.id === 'D')?.cause).toBe(null)
})

const SKILLS = [
  { name: 'review-and-draft-pr', description: 'Review the current change, resolve the findings, commit, push and open a draft pull request.' },
  { name: 'verify', description: 'Verification command' },
]

function longTurn(checks: LiveChecks, steps = 12) {
  for (let i = 0; i < steps; i += 1) checks.step(undefined, 'claude-opus-5', 'high', SMALL)
}

test('K1 matches a skill by name in words, by slash name, or by description keywords', () => {
  expect(matchSkill('please review and draft pr for this', SKILLS)?.name).toBe('review-and-draft-pr')
  expect(matchSkill('run /verify on the branch', SKILLS)?.name).toBe('verify')
  expect(matchSkill('resolve the findings and open a draft pull request', SKILLS)?.name).toBe('review-and-draft-pr')
  // A common word that is also a skill name is not a match.
  expect(matchSkill('verify the parser output', SKILLS)).toBe(null)
})

test('K1 knows a skill name after the first word is plain text', () => {
  expect(namesSkillLate('fix the empty state /verify', 'verify')).toBe(true)
  expect(namesSkillLate('/verify the branch', 'verify')).toBe(false)
  expect(namesSkillLate('please verify it', 'verify')).toBe(false)
  const checks = new LiveChecks()
  checks.prompt('fix the empty state /verify')
  longTurn(checks)
  checks.skills([{ name: 'verify', description: 'Run the checks' }])
  checks.turnEnd(undefined)
  const card = checks.takeView().checks.find(check => check.id === 'K1')
  expect(card?.metric).toBe('/verify named · not run')
  expect(card?.sentence).toMatch(/^You named this skill, but it did not run\./)
  expect(card?.prompt).not.toMatch(/\/verify.*\/verify/)
})

test('K1 fires after a long turn for a request that matches an unused skill, and Got it sets it quiet', () => {
  const checks = new LiveChecks()
  checks.prompt('review and draft pr for the parser change')
  longTurn(checks)
  expect(checks.needsSkills()).toBe(true)
  checks.skills(SKILLS)
  checks.turnEnd(undefined)
  const k1 = checks.takeView().checks.find(check => check.id === 'K1')
  expect(k1?.stage).toBe('active')
  expect(k1?.metric).toBe('/review-and-draft-pr fits · 12 steps')
  // The advice names only the skill. The request is not kept in the fix.
  expect(k1?.prompt).toBe('/review-and-draft-pr')
  expect(k1?.actionKind).toBe('advise')
  checks.acknowledge('K1')
  expect(stageOf(checks, 'K1')).toBe('quiet')
})

test('K1 does not look up skills for a short turn or after a skill ran', () => {
  const checks = new LiveChecks()
  checks.prompt('review and draft pr')
  longTurn(checks, 3)
  expect(checks.needsSkills()).toBe(false)
  checks.turnEnd(undefined)
  checks.prompt('review and draft pr again')
  longTurn(checks)
  checks.toolResult(undefined, 'Skill', { skill: 'review-and-draft-pr' }, false, false, 10)
  expect(checks.needsSkills()).toBe(false)
})

test('K1 still fires after an earlier request ran a slash command or a skill', () => {
  const checks = new LiveChecks()
  checks.prompt('/effort max')
  checks.turnEnd(undefined)
  checks.prompt('/model claude-opus-5')
  checks.toolResult(undefined, 'Skill', { skill: 'verify' }, false, false, 10)
  checks.turnEnd(undefined)
  checks.prompt('review and draft pr for the parser change')
  longTurn(checks)
  expect(checks.needsSkills()).toBe(true)
  checks.skills(SKILLS)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'K1')).toBe('active')
})

test('K1 resolves when a later request runs the skill', () => {
  const checks = new LiveChecks()
  checks.prompt('review and draft pr for the parser change')
  longTurn(checks)
  checks.skills(SKILLS)
  checks.turnEnd(undefined)
  checks.prompt('/review-and-draft-pr')
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'K1')).toBe('resolved')
})

test('K1 goes quiet when the person sends a new request', () => {
  const checks = new LiveChecks()
  checks.prompt('review and draft pr')
  longTurn(checks)
  checks.skills(SKILLS)
  checks.turnEnd(undefined)
  checks.prompt('now fix the flaky test')
  expect(stageOf(checks, 'K1')).toBe('quiet')
})

function edit(checks: LiveChecks, path: string) {
  checks.toolResult(undefined, 'Edit', { file_path: path }, false, false, 10)
}

test('C1 fires when edits after an approved plan reach files that the plan does not name', () => {
  const checks = new LiveChecks()
  checks.prompt('fix the parser')
  checks.planApproved('Change src/parser.rs and add a case to tests/parser.rs.')
  edit(checks, '/work/src/parser.rs')
  edit(checks, '/work/tests/parser.rs')
  edit(checks, '/work/src/lexer.rs')
  edit(checks, '/work/src/cli.rs')
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'C1')).toBe(undefined)
  edit(checks, '/work/README.md')
  checks.turnEnd(undefined)
  const c1 = checks.takeView().checks.find(check => check.id === 'C1')
  expect(c1?.stage).toBe('active')
  expect(c1?.metric).toBe('3 files outside the plan')
})

test('C1 stays silent without a plan, and a file that a later request names is in scope', () => {
  const checks = new LiveChecks()
  for (const path of ['/a.ts', '/b.ts', '/c.ts']) edit(checks, path)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'C1')).toBe(undefined)
  checks.planApproved('Change a.ts.')
  checks.prompt('also update b.ts and c.ts')
  for (const path of ['/a.ts', '/b.ts', '/c.ts']) edit(checks, path)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'C1')).toBe(undefined)
})

test('C1 resolves after its fix when three turns add no file outside the plan', () => {
  const checks = new LiveChecks()
  checks.planApproved('Change a.ts.')
  for (const path of ['/x.ts', '/y.ts', '/z.ts']) edit(checks, path)
  checks.turnEnd(undefined)
  const c1 = checks.takeView().checks.find(check => check.id === 'C1')
  checks.fillPressed('C1')
  expect(checks.prompt(c1?.prompt ?? '')).toBe(true)
  expect(stageOf(checks, 'C1')).toBe('watching')
  for (let i = 0; i < 3; i += 1) checks.turnEnd(undefined)
  expect(stageOf(checks, 'C1')).toBe('resolved')
})

function runCommands(checks: LiveChecks, commands: readonly string[]) {
  for (const command of commands) checks.toolResult(undefined, 'Bash', { command }, false, false, 10)
  checks.turnEnd(undefined)
}

test('K2 fires when the same set of commands runs in three turns, and resolves on a SKILL.md write', () => {
  const checks = new LiveChecks()
  const flow = ['cargo test --manifest-path a/Cargo.toml', 'npm run lint -- --fix', 'git commit -s -m x', 'rg -n foo']
  runCommands(checks, flow)
  runCommands(checks, flow)
  expect(stageOf(checks, 'K2')).toBe(undefined)
  runCommands(checks, flow)
  const k2 = checks.takeView().checks.find(check => check.id === 'K2')
  expect(k2?.metric).toBe('same 3 commands × 3')
  expect(k2?.prompt).toMatch(/repeated this session: cargo test, npm run lint, git commit\./)
  checks.toolResult(undefined, 'Write', { file_path: '/work/.claude/skills/ship/SKILL.md' }, false, false, 10)
  checks.turnEnd(undefined)
  expect(stageOf(checks, 'K2')).toBe('resolved')
})

test('K2 watches after its filled prompt is sent, and resolves as fixed on a SKILL.md write', () => {
  const checks = new LiveChecks()
  const flow = ['cargo test --manifest-path a/Cargo.toml', 'npm run lint -- --fix', 'git commit -s -m x']
  for (let i = 0; i < 3; i += 1) runCommands(checks, flow)
  const k2 = checks.takeView().checks.find(check => check.id === 'K2')
  checks.fillPressed('K2')
  // An edited prompt is a new request, so the check stays active.
  checks.prompt('Write a skill for my release steps.')
  expect(stageOf(checks, 'K2')).toBe('active')
  expect(checks.prompt(k2?.prompt ?? '')).toBe(true)
  expect(stageOf(checks, 'K2')).toBe('watching')
  checks.toolResult(undefined, 'Write', { file_path: '/work/.claude/skills/ship/SKILL.md' }, false, false, 10)
  checks.turnEnd(undefined)
  const resolved = checks.takeView().checks.find(check => check.id === 'K2')
  expect(resolved?.stage).toBe('resolved')
  expect(resolved?.isStoppedAfterFix).toBe(true)
})

test('K2 names commands by program and subcommand, and skips reads', () => {
  expect(commandName('FOO=1 cargo test -p x')).toBe('cargo test')
  expect(commandName('pnpm run build && echo ok')).toBe('pnpm run build')
  expect(commandName('cat a.ts')).toBe(null)
  expect(commandName('cd x')).toBe(null)
})

test('K1 ignores the context blocks that a host adds to the request', () => {
  const view = '<artifact-view-context artifact="a">{"slideTitle":"deck slide document"}</artifact-view-context>'
  const reminder = '<system-reminder>\nUse the docs skill for documents.\n</system-reminder>'
  expect(withoutHostBlocks(`${view}\nmake the slides one column ${reminder}`)).toBe('make the slides one column')
  // The words in the block do not count toward a skill match.
  const skills = [{ name: 'anthropic-skills:docs', description: 'Create and edit a document, deck or slide artifact', source: 'plugin' as const }]
  expect(matchSkill(withoutHostBlocks(`${view} fix it`), skills)).toBe(null)
  const checks = new LiveChecks()
  checks.prompt(`/verify ${reminder}`)
  // A request that starts with a command still counts as that command once the block goes.
  expect(namesSkillLate(withoutHostBlocks(`/verify ${reminder}`), 'verify')).toBe(false)
})
