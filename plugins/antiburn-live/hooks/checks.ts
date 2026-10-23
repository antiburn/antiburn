import type { LiveCheckId, LiveCheckView, LiveStage } from '../types'

import { CATALOG } from './catalog'

export type CheckId = LiveCheckId
export type CheckView = LiveCheckView

export type StepUsage = {
  input_tokens: number
  cache_read_input_tokens: number
  cache_creation_input_tokens: number
}

export type LiveView = {
  checks: CheckView[]
  newIds: CheckId[]
  /** The checks that resolved since the last view, in the order they resolved. */
  resolvedNow: CheckId[]
}

type Outcome =
  | { kind: 'hit'; metric: string; prompt?: string; cause?: string; sentence?: string; count: number }
  | { kind: 'building'; progress: string }
  | { kind: 'quiet' }
  | { kind: 'clear' }
  | { kind: 'none' }

type Entry = {
  stage: LiveStage
  metric: string
  prompt: string
  progress: string | null
  cause: string | null
  /** A sentence for this hit in place of the catalog sentence, or null. */
  sentence: string | null
  step: number
  watchLeft: number
  isHidden: boolean
  isStoppedAfterFix: boolean
}

const MAIN = 'main'
// K1 says this when the request names the skill but the name is not at the start.
const NAMED_SKILL_SENTENCE =
  'You named this skill, but it did not run. A skill name only runs at the start of a message.'
const LIVE = CATALOG.live
const POLICY = CATALOG.policy
const ABOVE_CAP: ReadonlySet<string> = new Set(POLICY.claude.effortAboveCap)
const REPLACEMENTS: Readonly<Record<string, string>> = POLICY.modelReplacements
const DEFINITIONS = [...CATALOG.checks].sort((a, b) => a.order - b.order)

/** Tools that only read files. A Bash call counts when core marks it read-only, or by its program. */
const READ_TOOLS: ReadonlySet<string> = new Set(['Read', 'Grep', 'Glob', 'LS', 'NotebookRead'])
const EDIT_TOOLS: ReadonlySet<string> = new Set(['Edit', 'MultiEdit', 'Write', 'NotebookEdit'])
const READ_PROGRAMS: ReadonlySet<string> = new Set([
  'cat', 'head', 'tail', 'less', 'nl', 'wc', 'ls', 'tree', 'find', 'fd', 'rg', 'grep', 'ag', 'sed',
])
/** A rough count of characters per token, for an estimate of the read output in the context. */
const CHARS_PER_TOKEN = 4

/**
 * Returns true when each part of a shell command only reads. `sed` counts only with `-n`
 * and without `-i`. A command with a redirect into a file does not count.
 */
export function isReadCommand(command: string): boolean {
  if (/>(?!&)/.test(command.replace(/2>&1|>\s*\/dev\/null/g, ''))) return false
  const parts = command.split(/&&|\|\||;|\|/).map(part => part.trim()).filter(part => part !== '')
  if (parts.length === 0) return false
  return parts.every(part => {
    const words = part.split(/\s+/)
    const program = words[0]?.replace(/^.*\//, '') ?? ''
    if (!READ_PROGRAMS.has(program)) return false
    if (program !== 'sed') return true
    return words.includes('-n') && !words.some(word => word.startsWith('-i'))
  })
}

/** One installed skill or command, as `$.command.list()` gives it. */
export type SkillInfo = { name: string; description: string }

/** Words too common to tell two skills apart. */
const STOP_WORDS: ReadonlySet<string> = new Set([
  'about', 'after', 'again', 'agent', 'any', 'before', 'claude', 'code', 'command', 'create', 'every',
  'files', 'first', 'from', 'into', 'make', 'other', 'should', 'skill', 'their', 'there', 'these',
  'thing', 'this', 'when', 'where', 'which', 'while', 'with', 'without', 'would', 'your', 'user',
  'users', 'using', 'asks', 'wants', 'something', 'anything', 'triggers', 'trigger',
])

function words(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9]+/).filter(word => word.length >= 5 && !STOP_WORDS.has(word))
}

/**
 * Gives the installed skill that a request matches, or null. A match needs the skill's
 * name in the request (`/name`, or a name with a dash in words), or 3 or more of the
 * keywords in its description. The gates are hypotheses until Phase 0 measures the noise.
 */
export function matchSkill(request: string, skills: readonly SkillInfo[]): SkillInfo | null {
  const lower = request.toLowerCase()
  const said = new Set(words(request))
  let best: { skill: SkillInfo; score: number } | null = null
  for (const skill of skills) {
    const name = skill.name.toLowerCase()
    const bare = name.slice(name.lastIndexOf(':') + 1)
    const isNamed =
      new RegExp(`(^|\\s)/${bare.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(\\s|$)`).test(lower) ||
      (bare.includes('-') && lower.includes(bare.replace(/-/g, ' ')))
    if (isNamed) return skill
    const score = new Set(words(skill.description).filter(word => said.has(word))).size
    if (score >= LIVE.skillMinKeywords && (best === null || score > best.score)) best = { skill, score }
  }
  return best?.skill ?? null
}

// Claude Code and its hosts add context blocks to a message, such as <system-reminder>.
// The person did not write them, so K1 does not match skills on them or send them again.
const HOST_BLOCK =
  /<(system-reminder|artifact-view-context|command-name|command-message|command-args|local-command-stdout|local-command-caveat)\b[^>]*>[\s\S]*?<\/\1>/g

/** Removes the context blocks that Claude Code and its hosts add to a message. */
export function withoutHostBlocks(text: string): string {
  return text.replace(HOST_BLOCK, '').trim()
}

/** Gives a pattern for `/name` as a word in a request. */
function slashPattern(name: string): RegExp {
  const bare = name.toLowerCase().slice(name.lastIndexOf(':') + 1)
  return new RegExp(`(^|\\s)/${bare.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?=\\s|$)`, 'gi')
}

/**
 * True when the request names the skill as `/name` after its first word. Claude Code runs
 * a skill name only at the start of a message, so a later name is plain text.
 */
export function namesSkillLate(request: string, name: string): boolean {
  const trimmed = request.trimStart()
  return !trimmed.toLowerCase().startsWith(`/${name.toLowerCase()}`) && slashPattern(name).test(trimmed)
}

/** Gives a short name for a shell command, such as `cargo test` or `npm run build`. */
export function commandName(command: string): string | null {
  const first = command.split(/&&|\|\||;|\|/)[0]?.trim() ?? ''
  const parts = first.split(/\s+/).filter(word => !/^[A-Z_]+=/.test(word))
  const program = parts[0]?.replace(/^.*\//, '') ?? ''
  if (program === '' || program === 'cd' || program === 'echo' || READ_PROGRAMS.has(program)) return null
  const rest = parts.slice(1).filter(word => !word.startsWith('-'))
  const depth = ['npm', 'pnpm', 'yarn', 'bun'].includes(program) && rest[0] === 'run' ? 2 : 1
  return [program, ...rest.slice(0, depth)].join(' ')
}

/** True for a test file, which a plan does not need to name. */
function isTestPath(path: string): boolean {
  return /(^|\/)(tests?|__tests__|spec)\/|\.(test|spec)\.[a-z]+$/.test(path)
}

/** Gives the file or folder that a read names, or null when the call names none. */
function readTarget(tool: string, input: Readonly<Record<string, unknown>>): string | null {
  for (const field of ['file_path', 'path', 'notebook_path']) {
    const value = input[field]
    if (typeof value === 'string' && value !== '') return value
  }
  if (tool === 'Grep' || tool === 'Glob' || tool === 'LS') return '.'
  if (tool !== 'Bash' || typeof input.command !== 'string') return null
  const words = input.command.split(/\s+/).slice(1)
  return words.find(word => !word.startsWith('-') && /[/.]/.test(word)) ?? null
}

/** Gives the same key as `canonical_model_key` in the Rust pricing module. */
export function canonicalModel(model: string): string {
  const trimmed = model.trim()
  const slash = trimmed.indexOf('/')
  let key = trimmed
  if (slash >= 0) key = trimmed.slice(slash + 1)
  else if (trimmed.startsWith('openai.')) key = trimmed.slice('openai.'.length)
  else if (trimmed.startsWith('antigravity-')) key = trimmed.slice('antigravity-'.length)
  const dated = /^(.+)-\d{8}$/.exec(key)
  if (dated?.[1] !== undefined && key.length > 9) key = dated[1]
  return key.toLowerCase()
}

export function replacementFor(model: string): string | undefined {
  return REPLACEMENTS[canonicalModel(model)]
}

function stepFor(count: number): number {
  return LIVE.retrySteps.filter(step => count >= step).length
}

function thousands(tokens: number): string {
  return `${Math.round(tokens / 1000)}k`
}

/**
 * Keeps the live check state for one session. Every method is synchronous
 * and does no I/O, so a hook can call it on each step at almost no cost.
 */
export class LiveChecks {
  private readonly entries = new Map<CheckId, Entry>()
  private readonly failures = new Map<string, Map<string, { count: number; tool: string }>>()
  private retryPeak = { count: 0, tool: '', thread: MAIN }
  private turns = 0
  private isTurnRunning = false
  private maxDepth = 0
  private lastDepth = 0
  private highReadings = 0
  private lastPercent = 0
  private mainModel = ''
  private lastEffort: string | null = null
  private effortRequests = 0
  private aboveCap = 0
  private topTier = ''
  /** E1 counts for the current main turn. */
  private turnReads = 0
  private turnEdits = 0
  private readonly turnPaths = new Set<string>()
  /** E2: the characters of read output in the main context since the last compact. */
  private readChars = 0
  /** K1: the current request, held in memory only, and the skills and commands that ran for it. */
  private request = ''
  private isNewRequest = false
  private turnSteps = 0
  private readonly skillsRun = new Set<string>()
  private skillMatch: SkillInfo | null = null
  private k1Skill = ''
  /** C1: the approved plan, the text that names allowed files, and the files outside it. */
  private planText: string | null = null
  private readonly outsidePlan = new Set<string>()
  private turnOutside = 0
  private planCleanTurns = 0
  /** K2: the commands of the current turn, and how often each set of commands ran. */
  private readonly turnCommands: string[] = []
  private readonly workflows = new Map<string, number>()
  private workflowPeak: { commands: string; count: number } | null = null
  private hasSkillFile = false
  private newIds = new Set<CheckId>()
  /** Fill actions that the person pressed but whose result no request shows yet. */
  private readonly pendingFills = new Set<CheckId>()
  private resolvedNow: CheckId[] = []
  /** Increments each time the visible state changes. */
  version = 0

  toolCall(thread: string | undefined, tool: string, key: string, isError: boolean): void {
    const id = thread ?? MAIN
    let calls = this.failures.get(id)
    if (calls === undefined) {
      calls = new Map()
      this.failures.set(id, calls)
    }
    if (!isError) {
      calls.delete(key)
      return
    }
    const seen = calls.get(key)
    calls.set(key, { count: (seen?.count ?? 0) + 1, tool })
  }

  /** Records one finished tool call for E1 and E2. Only the main thread fills the main context. */
  toolResult(
    thread: string | undefined,
    tool: string,
    input: Readonly<Record<string, unknown>>,
    isReadOnly: boolean,
    isError: boolean,
    resultChars: number,
  ): void {
    if (thread !== undefined) return
    if (tool === 'Skill' && !isError && typeof input.skill === 'string') this.skillsRun.add(input.skill.toLowerCase())
    if (EDIT_TOOLS.has(tool)) {
      if (!isError) this.edited(input)
      return
    }
    if (tool === 'Bash' && !isError && typeof input.command === 'string') {
      const name = commandName(input.command)
      if (name !== null && !this.turnCommands.includes(name)) this.turnCommands.push(name)
    }
    const command = typeof input.command === 'string' ? input.command : ''
    const isRead = READ_TOOLS.has(tool) || (tool === 'Bash' && (isReadOnly || isReadCommand(command)))
    if (!isRead) return
    this.readChars += resultChars
    if (isError) return
    this.turnReads += 1
    const target = readTarget(tool, input)
    if (target !== null && this.turnPaths.size < 1000) this.turnPaths.add(target)
  }

  private edited(input: Readonly<Record<string, unknown>>): void {
    this.turnEdits += 1
    const path = typeof input.file_path === 'string' ? input.file_path : typeof input.notebook_path === 'string' ? input.notebook_path : ''
    if (path === '') return
    if (/\/skills\/[^/]+\/SKILL\.md$/.test(path)) this.hasSkillFile = true
    if (this.planText === null || isTestPath(path) || this.outsidePlan.has(path)) return
    const base = path.slice(path.lastIndexOf('/') + 1)
    if (this.planText.includes(base)) return
    this.outsidePlan.add(path)
    this.turnOutside += 1
  }

  /** Records an approved plan. Files that the plan or a later request names count as in the plan. */
  planApproved(plan: string): void {
    this.planText = `${plan}\n${this.request}`
    this.outsidePlan.clear()
    this.planCleanTurns = 0
  }

  /**
   * Records a request from the person. A check's own prompt does not count as a new
   * request. Returns true when the view changed.
   */
  prompt(message: string): boolean {
    const text = withoutHostBlocks(message)
    const own = DEFINITIONS.some(definition => definition.prompt === text)
    // K1 asks about the skills of this request only, so a new request starts with none.
    if (!own) this.skillsRun.clear()
    const slash = /^\/([^\s]+)/.exec(text)?.[1]?.toLowerCase()
    if (slash !== undefined) {
      this.skillsRun.add(slash)
      if (this.pendingFills.has('K1') && slash === this.k1Skill) this.fillTookEffect('K1')
    }
    if (this.pendingFills.has('C1') && text.trim() === (DEFINITIONS.find(check => check.id === 'C1')?.prompt ?? '')) {
      this.fillTookEffect('C1')
      return true
    }
    // The K2 prompt names the repeated commands, so it matches the check's own prompt, not the template.
    const workflow = this.entries.get('K2')
    if (this.pendingFills.has('K2') && workflow !== undefined && text.trim() === workflow.prompt.trim()) {
      this.fillTookEffect('K2')
      return true
    }
    if (own) return false
    this.request = text.slice(0, 2000)
    this.isNewRequest = true
    if (this.planText !== null) this.planText += `\n${this.request}`
    const before = this.version
    // A new request changes the task, so the checks about the last request go quiet.
    if (this.entries.get('K1')?.stage === 'active') this.apply('K1', { kind: 'quiet' })
    if (this.entries.get('C1')?.stage === 'active') {
      this.apply('C1', { kind: 'quiet' })
      this.outsidePlan.clear()
    }
    return this.version !== before
  }

  /**
   * True when K1 needs the installed skills: a long turn for a request that names no
   * skill and ran none. The caller then reads `$.command.list()` once and calls `skills`.
   */
  needsSkills(): boolean {
    return (
      this.isNewRequest &&
      this.turnSteps >= LIVE.skillMinSteps &&
      this.request !== '' &&
      !this.request.startsWith('/') &&
      this.skillsRun.size === 0
    )
  }

  skills(list: readonly SkillInfo[]): void {
    this.skillMatch = matchSkill(this.request, list)
  }

  step(thread: string | undefined, model: string, effort: unknown, usage: StepUsage | null): void {
    if (thread !== undefined) return
    this.isTurnRunning = true
    this.turnSteps += 1
    this.mainModel = model
    if (usage !== null) {
      const depth =
        usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens
      this.lastDepth = depth
      this.maxDepth = Math.max(this.maxDepth, depth)
    }
    if (this.pendingFills.has('O') && replacementFor(model) === undefined) this.fillTookEffect('O')
    // A numeric effort has no tier name, so T does not count it.
    if (typeof effort !== 'string') return
    if (this.pendingFills.has('T') && !ABOVE_CAP.has(effort)) this.fillTookEffect('T')
    this.lastEffort = effort
    this.effortRequests += 1
    if (ABOVE_CAP.has(effort)) {
      this.aboveCap += 1
      this.topTier = effort
    }
  }

  /** Records the end of one thread's turn. Returns true when the view changed. */
  turnEnd(thread: string | undefined): boolean {
    const id = thread ?? MAIN
    for (const call of this.failures.get(id)?.values() ?? []) {
      if (call.count > this.retryPeak.count) this.retryPeak = { ...call, thread: id }
    }
    this.failures.delete(id)
    if (thread !== undefined) return false
    this.turns += 1
    this.isTurnRunning = false
    const before = this.version
    this.apply('X1', this.retryOutcome())
    this.apply('E1', this.readingOutcome())
    this.apply('D', this.contextOutcome())
    this.apply('O', this.modelOutcome())
    this.apply('T', this.effortOutcome())
    this.apply('K1', this.skillOutcome())
    this.apply('C1', this.planOutcome())
    this.apply('K2', this.workflowOutcome())
    this.retryPeak = { count: 0, tool: '', thread: MAIN }
    this.maxDepth = 0
    this.turnReads = 0
    this.turnEdits = 0
    this.turnPaths.clear()
    this.turnSteps = 0
    this.turnOutside = 0
    this.turnCommands.length = 0
    this.skillMatch = null
    this.isNewRequest = false
    return this.version !== before
  }

  measure(percent: number | undefined): boolean {
    if (percent === undefined) return false
    this.lastPercent = percent
    this.highReadings = percent >= LIVE.contextPercent ? this.highReadings + 1 : 0
    if (this.isTurnRunning) return false
    const before = this.version
    this.apply('D', this.contextOutcome())
    return this.version !== before
  }

  compact(): boolean {
    this.maxDepth = 0
    this.lastDepth = 0
    this.highReadings = 0
    this.readChars = 0
    const before = this.version
    this.apply('D', { kind: 'clear' })
    return this.version !== before
  }

  fixSent(id: CheckId): void {
    const entry = this.entries.get(id)
    if (entry === undefined || entry.stage !== 'active') return
    entry.stage = 'watching'
    entry.watchLeft = LIVE.watchTurns
    this.version += 1
  }

  /** Returns a watched check to active when its fix did not run. */
  unwatch(id: CheckId): void {
    const entry = this.entries.get(id)
    if (entry === undefined || entry.stage !== 'watching') return
    entry.stage = 'active'
    this.version += 1
  }

  /**
   * Records a press on a fill action. The person can still edit or discard the
   * filled text, so watching starts only when a request shows the change.
   */
  fillPressed(id: CheckId): void {
    if (this.entries.get(id)?.stage === 'active') this.pendingFills.add(id)
  }

  private fillTookEffect(id: CheckId): void {
    this.pendingFills.delete(id)
    this.fixSent(id)
  }

  /** Sets an advice check quiet after the person reads it. A later match shows it again. */
  acknowledge(id: CheckId): void {
    if (this.entries.get(id)?.stage === 'active') this.apply(id, { kind: 'quiet' })
  }

  hide(id: CheckId): void {
    const entry = this.entries.get(id)
    if (entry === undefined || entry.isHidden) return
    entry.isHidden = true
    this.version += 1
  }

  /** Returns the view and clears the one-time `newIds` and `resolvedNow` values. */
  takeView(): LiveView {
    const checks: CheckView[] = []
    for (const definition of DEFINITIONS) {
      const entry = this.entries.get(definition.id)
      if (entry === undefined) continue
      checks.push({
        id: definition.id,
        label: definition.label,
        area: definition.area,
        sentence: entry.sentence ?? definition.sentence,
        stage: entry.stage,
        metric: entry.metric,
        progress: entry.progress,
        cause: entry.cause,
        actionKind: definition.action.kind,
        actionLabel: definition.action.label,
        prompt: entry.prompt,
        isHidden: entry.isHidden,
        isStoppedAfterFix: entry.isStoppedAfterFix,
      })
    }
    const view = { checks, newIds: [...this.newIds], resolvedNow: [...this.resolvedNow] }
    this.newIds = new Set()
    this.resolvedNow = []
    return view
  }

  private retryOutcome(): Outcome {
    const { count, tool, thread } = this.retryPeak
    if (count < LIVE.retryMinRepeats) return { kind: 'clear' }
    const where = thread === MAIN ? '' : ' · subagent'
    return { kind: 'hit', metric: `${count} tries · ${tool}${where}`, count }
  }

  private skillOutcome(): Outcome {
    const entry = this.entries.get('K1')
    if (entry !== undefined && this.skillsRun.has(this.k1Skill)) return { kind: 'clear' }
    const match = this.skillMatch
    if (match === null) return { kind: 'none' }
    this.k1Skill = match.name.toLowerCase()
    const definition = DEFINITIONS.find(check => check.id === 'K1')
    const isNamed = namesSkillLate(this.request, match.name)
    return {
      kind: 'hit',
      metric: isNamed ? `/${match.name} named · not run` : `/${match.name} fits · ${this.turnSteps} steps`,
      prompt: (definition?.prompt ?? '').replace('{skill}', match.name),
      ...(isNamed ? { sentence: NAMED_SKILL_SENTENCE } : {}),
      count: 1,
    }
  }

  private planOutcome(): Outcome {
    if (this.planText === null) return { kind: 'none' }
    const stage = this.entries.get('C1')?.stage
    if (stage === 'active' || stage === 'watching') {
      if (this.turnOutside > 0) return this.planHit()
      this.planCleanTurns += 1
      return this.planCleanTurns >= LIVE.planCleanTurns ? { kind: 'clear' } : { kind: 'none' }
    }
    if (this.outsidePlan.size < LIVE.planMinFiles) return { kind: 'none' }
    return this.planHit()
  }

  private planHit(): Outcome {
    this.planCleanTurns = 0
    return { kind: 'hit', metric: `${this.outsidePlan.size} files outside the plan`, count: 1 }
  }

  private workflowOutcome(): Outcome {
    if (this.hasSkillFile && this.entries.has('K2')) return { kind: 'clear' }
    if (this.turnCommands.length >= LIVE.workflowMinCommands) {
      const key = this.turnCommands.join(', ')
      const count = (this.workflows.get(key) ?? 0) + 1
      if (this.workflows.size < 200 || this.workflows.has(key)) this.workflows.set(key, count)
      if (count >= LIVE.workflowMinRepeats && count > (this.workflowPeak?.count ?? 0)) {
        this.workflowPeak = { commands: key, count }
      }
    }
    const peak = this.workflowPeak
    if (peak === null) return { kind: 'none' }
    const definition = DEFINITIONS.find(check => check.id === 'K2')
    return {
      kind: 'hit',
      metric: `same ${peak.commands.split(', ').length} commands × ${peak.count}`,
      prompt: (definition?.prompt ?? '').replace('{commands}', peak.commands),
      count: 1,
    }
  }

  private readingOutcome(): Outcome {
    if (this.turnEdits > 0) return { kind: 'clear' }
    const paths = this.turnPaths.size
    if (this.turnReads < LIVE.readMinCalls || paths < LIVE.readMinPaths) return { kind: 'none' }
    return { kind: 'hit', metric: `${this.turnReads} reads · 0 edits · ${paths} files`, count: 1 }
  }

  private contextOutcome(): Outcome {
    const cap = POLICY.depthCapTokens
    const depth = Math.max(this.maxDepth, this.lastDepth)
    if (depth > cap) {
      return { kind: 'hit', metric: `${thousands(depth)} tokens per request`, count: 1, ...this.readCause(depth) }
    }
    if (this.highReadings >= LIVE.contextReadings) {
      return { kind: 'hit', metric: `${this.lastPercent}% of context used`, count: 1, ...this.readCause(depth) }
    }
    return { kind: 'clear' }
  }

  /**
   * E2: names file reads as the cause of a heavy context when their output is most of it.
   * The share is an estimate from the characters of read output, so the line says "about".
   */
  private readCause(depth: number): { cause?: string; prompt?: string } {
    const tokens = this.readChars / CHARS_PER_TOKEN
    if (depth <= 0 || tokens / depth < LIVE.readMinShare) return {}
    const share = Math.min(99, Math.round((tokens / depth) * 100))
    const definition = DEFINITIONS.find(check => check.id === 'D')
    const extra = definition !== undefined && 'readsPrompt' in definition ? definition.readsPrompt : ''
    return {
      cause: `File reads are about ${share}% of it (${thousands(tokens)} tokens).`,
      prompt: `${definition?.prompt ?? ''}${extra}`,
    }
  }

  private modelOutcome(): Outcome {
    if (this.mainModel === '') return { kind: 'none' }
    const replacement = replacementFor(this.mainModel)
    if (replacement === undefined) return { kind: 'clear' }
    // A switch is cheap only before the context grows (plan decision 2).
    if (this.turns > 1) return { kind: 'quiet' }
    const definition = DEFINITIONS.find(check => check.id === 'O')
    return {
      kind: 'hit',
      metric: `${shortModel(this.mainModel)} → ${shortModel(replacement)}`,
      prompt: (definition?.prompt ?? '').replace('{replacement}', replacement),
      count: 1,
    }
  }

  private effortOutcome(): Outcome {
    if (this.effortRequests === 0 || this.aboveCap === 0) return { kind: 'none' }
    if (this.lastEffort !== null && !ABOVE_CAP.has(this.lastEffort)) return { kind: 'clear' }
    const share = this.aboveCap / this.effortRequests
    const need = LIVE.effortMinRequests
    if (this.turns >= LIVE.graceTurns && this.aboveCap >= need && share >= LIVE.effortMinShare) {
      return {
        kind: 'hit',
        metric: `${this.aboveCap} of ${this.effortRequests} requests at ${this.topTier}`,
        count: 1,
      }
    }
    // The progress names the first condition that is not met. When only the share is low, more
    // turns do not help, so the progress shows the share.
    const percent = (value: number) => `${Math.round(value * 100)}%`
    const progress =
      this.aboveCap < need
        ? `${this.aboveCap} of ${need} requests`
        : this.turns < LIVE.graceTurns
          ? `turn ${this.turns} of ${LIVE.graceTurns}`
          : `${percent(share)} of requests at ${this.topTier}, needs ${percent(LIVE.effortMinShare)}`
    return { kind: 'building', progress }
  }

  private apply(id: CheckId, outcome: Outcome): void {
    const entry = this.entries.get(id)
    const definition = DEFINITIONS.find(check => check.id === id)
    const basePrompt = definition?.prompt ?? ''
    if (outcome.kind === 'none') return
    if (outcome.kind === 'hit') {
      const prompt = outcome.prompt ?? basePrompt
      const cause = outcome.cause ?? null
      const step = stepFor(outcome.count)
      if (entry === undefined) {
        this.entries.set(id, {
          stage: 'active',
          metric: outcome.metric,
          prompt,
          progress: null,
          cause,
          sentence: outcome.sentence ?? null,
          step,
          watchLeft: 0,
          isHidden: false,
          isStoppedAfterFix: false,
        })
        this.mark(id)
        return
      }
      const isStepUp = step > entry.step
      const wasLive = entry.stage === 'active' || entry.stage === 'watching'
      const isSame =
        entry.stage === 'active' &&
        entry.metric === outcome.metric &&
        entry.prompt === prompt &&
        entry.cause === cause &&
        entry.sentence === (outcome.sentence ?? null)
      if (isSame && !isStepUp) return
      entry.metric = outcome.metric
      entry.prompt = prompt
      entry.cause = cause
      entry.sentence = outcome.sentence ?? null
      entry.progress = null
      entry.step = Math.max(entry.step, step)
      if (entry.stage === 'watching') {
        entry.watchLeft -= 1
        if (entry.watchLeft <= 0) entry.stage = 'active'
      } else {
        entry.stage = 'active'
      }
      if (!wasLive || isStepUp) this.mark(id)
      this.version += 1
      return
    }
    if (outcome.kind === 'building') {
      if (entry !== undefined && entry.stage !== 'building') return
      if (entry?.progress === outcome.progress) return
      this.entries.set(id, {
        stage: 'building',
        metric: '',
        prompt: basePrompt,
        progress: outcome.progress,
        cause: null,
        sentence: null,
        step: 0,
        watchLeft: 0,
        isHidden: entry?.isHidden ?? false,
        isStoppedAfterFix: false,
      })
      this.version += 1
      return
    }
    if (entry === undefined) return
    if (outcome.kind === 'quiet') {
      if (entry.stage !== 'active' && entry.stage !== 'watching') return
      entry.stage = 'quiet'
      this.version += 1
      return
    }
    if (entry.stage === 'building') {
      this.entries.delete(id)
      this.version += 1
      return
    }
    if (entry.stage === 'resolved') return
    entry.isStoppedAfterFix = entry.stage === 'watching'
    entry.stage = 'resolved'
    this.resolvedNow.push(id)
    this.version += 1
  }

  private mark(id: CheckId): void {
    this.newIds.add(id)
    this.version += 1
  }
}

/** The checks that the strip and the chip count (plan §12.5). */
export function liveChecks(view: LiveView): CheckView[] {
  return view.checks.filter(
    check => !check.isHidden && (check.stage === 'active' || check.stage === 'watching'),
  )
}

const DEMO_METRICS: Readonly<Record<(typeof DEFINITIONS)[number]['id'], string>> = {
  X1: '3 tries · Bash',
  E1: '24 reads · 0 edits · 17 files',
  D: '182k tokens per request',
  O: 'sonnet-4 → sonnet-4-5',
  T: '6 of 6 requests at max',
  K1: '/review-and-draft-pr fits · 14 steps',
  C1: '4 files outside the plan',
  K2: 'same 3 commands × 3',
}

const DEMO_FILLS: Readonly<Record<string, string>> = {
  '{replacement}': 'claude-sonnet-4-5',
  '{skill}': 'review-and-draft-pr',
  '{commands}': 'cargo test, npm run lint, git commit',
}

/**
 * The sample view for a visual test of the pane. It shows each built check once and
 * each stage that the list shows: a fill, a submit and a compact fix, a watched check,
 * a check that its fix resolved and a check that resolved by itself. The pane hides building checks, so no card builds.
 */
export function demoView(): CheckView[] {
  const stages: Record<(typeof DEFINITIONS)[number]['id'], LiveStage> = {
    X1: 'watching',
    E1: 'active',
    D: 'active',
    O: 'resolved',
    T: 'active',
    K1: 'active',
    C1: 'resolved',
    K2: 'active',
  }
  return DEFINITIONS.map(definition => {
    const stage = stages[definition.id]
    return {
      id: definition.id,
      label: definition.label,
      area: definition.area,
      sentence: definition.sentence,
      stage,
      metric: DEMO_METRICS[definition.id],
      progress: null,
      cause: definition.id === 'D' ? 'File reads are about 61% of it (111k tokens).' : null,
      actionKind: definition.action.kind,
      actionLabel: definition.action.label,
      prompt: Object.entries(DEMO_FILLS).reduce<string>((text, [mark, value]) => text.replace(mark, value), definition.prompt ?? ''),
      isHidden: false,
      // O shows a resolve by its fix, C1 a resolve by itself.
      isStoppedAfterFix: stage === 'resolved' && definition.id === 'O',
    }
  })
}

// The card metric drops the "claude-" prefix, so the model pair fits a narrow pane.
// The detail and the /model command keep the full ID.
export function shortModel(model: string): string {
  return model.replace(/^claude-/, '')
}
