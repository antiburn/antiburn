import type { AgentMemoriesReport, MemoryEntry, MemoryProject } from "../../../lib/memoriesIpc"

export function entry(over: Partial<MemoryEntry> = {}): MemoryEntry {
  return {
    path: "/p/memory/a.md",
    fileName: "a.md",
    title: "Alpha",
    kind: "feedback",
    hook: "Alpha hook",
    hookSource: "index",
    indexEntry: { title: "Alpha", hook: "Alpha hook", lineNumber: 1 },
    frontmatter: "name: Alpha\ndescription: Alpha description\ntype: feedback",
    body: "Alpha body",
    truncated: false,
    sizeBytes: 2048,
    modifiedMs: null,
    inIndex: true,
    facts: {
      lastReferencedMs: null,
      lastWrittenMs: null,
      referenceCount: 0,
      writeCount: 0,
      sessionsSinceWritten: null,
      hasHistory: true,
    },
    ...over,
  }
}

export function project(over: Partial<MemoryProject> = {}): MemoryProject {
  return {
    agent: "claude-code",
    slug: "-p",
    displayPath: "~/p",
    folderExists: true,
    memoryDir: "/p/memory",
    indexPath: "/p/memory/MEMORY.md",
    sessionCount: 3,
    lastSessionMs: null,
    dangling: [],
    memories: [entry()],
    ...over,
  }
}

export function report(projects: MemoryProject[] = [project()]): AgentMemoriesReport {
  return { generatedAtMs: 1_000_000, writesSupported: true, projects }
}
