import { Brain, LoaderCircle } from "lucide-react"
import { useSyncExternalStore } from "react"

import { Disclosure } from "../../../components/ui/Disclosure"
import { HeroFigures, type HeroFigureCell } from "../../../components/ui/HeroFigures"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { cn } from "../../../lib/cn"
import { detectPlatform } from "../../../lib/platform"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { AgentMemoriesReport } from "../../../lib/memoriesIpc"
import type { MemoriesSnapshot } from "./MemoriesSession"
import type { MemoriesSession } from "./MemoriesSession"
import { MemoryProjectGroup } from "./MemoryProjectGroup"

const USED_WINDOW_MS = 7 * 24 * 3_600_000

function memoryTotals(
  projects: AgentMemoriesReport["projects"],
  state: Pick<MemoriesSnapshot, "archived">,
  now: number,
) {
  let memories = 0
  let used = 0
  const agents = new Set<string>()
  for (const project of projects) {
    agents.add(project.agent)
    for (const memory of project.memories) {
      if (state.archived.has(memory.path)) continue
      memories += 1
      const read = memory.facts.lastReferencedMs
      if (read != null && now - read <= USED_WINDOW_MS) used += 1
    }
  }
  return { memories, projects: projects.length, used, agents: [...agents].sort() }
}

function MemoriesEmptyState() {
  const computer = detectPlatform() === "macos" ? "Mac" : "computer"
  return (
    <div
      role="status"
      className="flex flex-1 flex-col items-center justify-center px-8 py-12 text-center"
    >
      <Brain size={28} aria-hidden className="mb-3 text-label-tertiary" />
      <p className="type-body text-label">No agent memories found</p>
      <p className="mt-1 type-callout text-label-tertiary">
        {`Claude Code stores memories under ~/.claude/projects/<project>/memory. None were found on this ${computer}.`}
      </p>
    </div>
  )
}

function Heading({ children }: { children: string }) {
  return <span className="font-medium text-label">{children} </span>
}

function Code({ children }: { children: string }) {
  return <code className="font-mono type-footnote">{children}</code>
}

function MemoriesExplainer() {
  return (
    <section
      aria-label="About memories"
      className="session-card rounded-(--radius-popover) bg-session-card px-4 py-3 flex flex-col gap-y-3"
    >
      <p className="type-callout text-label-secondary max-w-180">
        Claude automatically keeps notes about each project as memory files, listed in a
        MEMORY.md file.
      </p>
      <p className="type-callout text-label-secondary max-w-180">
        MEMORY.md is loaded into every session, which costs tokens, but the memory files
        themselves are only read when Claude decides they might be useful.
      </p>
      <p className="type-callout text-label-secondary max-w-180">
        Memories cost a small amount of tokens, but more importantly they can confuse the agent.
        When looking at your memories, you'll often see things that aren't relevant any more.
        Unfortunately Claude might still load them sometimes, and confuse itself, and give you
        worse quality work.
      </p>
      <p className="type-callout text-label-secondary max-w-180">
        So it's usually worth tidying up your memories every month or two.
      </p>

      <Disclosure label="More about how memories work" className="-mt-3">
        <div className="grid gap-y-3 type-footnote text-label-secondary max-w-180">
          <p>
            <Heading>Which agents.</Heading>
            Claude is the only agent at the moment that stores memories automatically like this.
          </p>
          <p>
            <Heading>Where they live.</Heading>
            <Code>~/.claude/projects/&lt;project&gt;/memory/</Code>, one Markdown file per
            memory plus <Code>MEMORY.md</Code>. Claude writes them during sessions.
          </p>
          <p>
            <Heading>What Claude sees.</Heading>
            At the start of every session Claude injects <Code>MEMORY.md</Code>. Each line is a
            link with a title and a short hook, and that's all Claude knows about a memory until
            it reads it. A memory with no index line is invisible to Claude, and an index line
            with a missing file wastes context.
          </p>
          <p>
            <Heading>What's in a file.</Heading>A frontmatter block (<Code>name</Code>,{" "}
            <Code>description</Code>, <Code>type</Code>) and a body. The description is for
            Claude&apos;s own search over memory files. The body is only loaded when Claude
            opens the file. The index hook, the description and the body are written at
            different times and can disagree; the index is the one Claude reads first.
          </p>
          <p>
            <Heading>Kinds of memory.</Heading>
            <Code>user</Code> is about who you are. <Code>feedback</Code> is about how you want
            Claude to work. <Code>project</Code> is about ongoing work and decisions.{" "}
            <Code>reference</Code> is about pointers to external resources.
          </p>
          <p>
            <Heading>Usage.</Heading>
            Referenced is the last stored session that read the file or named it in a Bash
            command. Written is the last Write or Edit. Sessions since is how many sessions in
            this project started since the last write. We might not always have old transcripts
            if they've been tidied up, so it's possible a memory was read more than 30 days ago.
          </p>
          <p>
            <Heading>Deleting.</Heading>
            Delete moves the file to antiburn&apos;s archive and removes its index line, with a
            backup of <Code>MEMORY.md</Code>. Undo puts both back.
          </p>
        </div>
      </Disclosure>
    </section>
  )
}

function heroCells(
  totals: ReturnType<typeof memoryTotals> | null,
  generatedAtMs: number,
  now: number,
): HeroFigureCell[] {
  const t = totals ?? { memories: 0, projects: 0, used: 0, agents: [] }
  const usedPercent = t.memories === 0 ? 0 : (t.used * 100) / t.memories
  return [
    {
      key: "agents",
      label: "Agents with Memories",
      figure: <SegmentFigure>{String(t.agents.length)}</SegmentFigure>,
      caption: t.agents.length === 0 ? "none found" : t.agents.map(agentDisplayName).join(", "),
    },
    {
      key: "projects",
      label: "Projects",
      figure: <SegmentFigure>{String(t.projects)}</SegmentFigure>,
      caption: "with a memory folder",
    },
    {
      key: "memories",
      label: "Memories",
      figure: <SegmentFigure>{String(t.memories)}</SegmentFigure>,
      caption: `updated ${relativeTime(new Date(generatedAtMs).toISOString(), { now })}`,
    },
    {
      key: "used",
      label: "Used recently",
      figure: <SegmentFigure>{`${usedPercent.toFixed(1)}%`}</SegmentFigure>,
      caption: "read in the last 7 days",
      tooltip:
        "Counts Read and Bash tool calls that named the memory file in a stored Claude Code session. A write alone does not count: it only shows an agent guessed the memory might be needed.",
    },
  ]
}

/**
 * The main window's Memories section: every project's Claude Code memories
 * with what the stored tool calls say about their use. A memory can be deleted
 * (moved to the archive, with Undo) and a dangling index line removed.
 */
export function MemoriesView({
  active,
  session,
}: {
  active: boolean
  session: MemoriesSession
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const { report } = state
  const now = session.now()
  const totals = report ? memoryTotals(report.projects, state, now) : null

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window">
      <h1 className="sr-only">Memories</h1>
      {state.error ? (
        <div className="flex flex-1 items-center justify-center text-center">
          <div>
            <p role="alert" className="type-body text-label-secondary">
              Memories are unavailable.
            </p>
            <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
              Retry
            </button>
          </div>
        </div>
      ) : !report ? (
        <div
          role="status"
          aria-busy="true"
          className="flex flex-1 flex-col items-center justify-center px-8 py-12 text-center"
        >
          <LoaderCircle
            size={28}
            strokeWidth={2}
            aria-hidden="true"
            className="mb-3 animate-spin text-label-tertiary"
          />
          <p className="type-body text-label-secondary">Loading memories</p>
        </div>
      ) : report.projects.length === 0 ? (
        <MemoriesEmptyState />
      ) : (
        <ScrollPane className="min-h-0" viewportLabel="Memories">
          <div
            aria-busy={state.loading || undefined}
            className={cn(
              "@container grid gap-y-6 px-8 pt-6 pb-16 transition-opacity duration-[var(--duration-medium)]",
              state.loading && "opacity-60",
            )}
          >
            <section aria-label="Memory totals">
              <HeroFigures cells={heroCells(totals, report.generatedAtMs, now)} />
            </section>
            <MemoriesExplainer />
            <div className="grid gap-y-1.5">
              {report.projects.map((project) => (
                <MemoryProjectGroup
                  key={project.slug}
                  project={project}
                  collapsed={state.collapsedProjects.has(project.slug)}
                  state={state}
                  writesSupported={report.writesSupported}
                  now={now}
                  onToggleProject={() => session.toggleProject(project.slug)}
                  onToggleMemory={session.toggleMemory}
                  onReveal={(path) => void session.reveal(path)}
                  onDelete={(entry) => void session.archive(project, entry)}
                  onUndo={(entry) => void session.undo(entry)}
                  onRemoveLine={(entry) => void session.removeIndexLine(project, entry)}
                  onReload={session.refresh}
                  onFocusHandled={session.focusHandled}
                />
              ))}
            </div>
          </div>
        </ScrollPane>
      )}
    </div>
  )
}
