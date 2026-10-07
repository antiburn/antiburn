import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

/** Mirrors Rust `AgentMemoriesReport`. */
export interface AgentMemoriesReport {
  generatedAtMs: number
  projects: MemoryProject[]
}

/** Mirrors Rust `MemoryProjectDto`. */
export interface MemoryProject {
  slug: string
  displayPath: string
  memoryDir: string
  indexPath: string | null
  sessionCount: number
  lastSessionMs: number | null
  dangling: DanglingIndexEntry[]
  memories: MemoryEntry[]
}

/** Mirrors Rust `DanglingIndexEntryDto`. */
interface DanglingIndexEntry {
  title: string
  target: string
  lineNumber: number
}

/** Mirrors Rust `MemoryEntryDto`. */
export interface MemoryEntry {
  path: string
  fileName: string
  title: string
  kind: string | null
  hook: string | null
  hookSource: "index" | "frontmatter" | "body"
  body: string
  hasFrontmatter: boolean
  truncated: boolean
  sizeBytes: number
  modifiedMs: number | null
  inIndex: boolean
  facts: MemoryFacts
}

/** Mirrors Rust `MemoryFactsDto`. */
interface MemoryFacts {
  lastReferencedMs: number | null
  lastWrittenMs: number | null
  referenceCount: number
  writeCount: number
  sessionsSinceWritten: number | null
  hasHistory: boolean
}

/** Every Claude Code auto-memory project. Empty outside the desktop shell. */
export async function listAgentMemories(): Promise<AgentMemoriesReport> {
  if (!hasShell()) return { generatedAtMs: Date.now(), projects: [] }
  return invoke<AgentMemoriesReport>("list_agent_memories")
}
