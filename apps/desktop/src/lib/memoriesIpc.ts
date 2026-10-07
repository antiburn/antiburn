import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

/** Mirrors Rust `AgentMemoriesReport`. */
export interface AgentMemoriesReport {
  generatedAtMs: number
  /** False where the memory editor cannot write (Windows). */
  writesSupported: boolean
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
export interface DanglingIndexEntry {
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
  if (!hasShell()) return { generatedAtMs: Date.now(), writesSupported: false, projects: [] }
  return invoke<AgentMemoriesReport>("list_agent_memories")
}

/** Mirrors Rust `MemoryEditOutcome`. Only the first three outcomes changed a
 *  file; the others left the memory folder untouched. */
export type MemoryEditOutcome =
  | { outcome: "archived"; archiveId: string; indexLineRemoved: boolean }
  | { outcome: "restored"; indexLineRestored: boolean }
  | { outcome: "indexLineRemoved" }
  | { outcome: "changedOnDisk" }
  | { outcome: "missing" }
  | { outcome: "alreadyExists" }
  | { outcome: "unavailable"; reason: string }

const NO_SHELL: MemoryEditOutcome = { outcome: "unavailable", reason: "noShell" }

/** Move a memory file into antiburn's archive. The size and time must match
 *  the file the report listed. */
export async function archiveAgentMemory(
  slug: string,
  fileName: string,
  expectedSizeBytes: number,
  expectedModifiedMs: number | null,
): Promise<MemoryEditOutcome> {
  if (!hasShell()) return NO_SHELL
  return invoke<MemoryEditOutcome>("archive_agent_memory", {
    slug,
    fileName,
    expectedSizeBytes,
    expectedModifiedMs,
  })
}

/** Put an archived memory back. */
export async function restoreAgentMemory(
  slug: string,
  archiveId: string,
): Promise<MemoryEditOutcome> {
  if (!hasShell()) return NO_SHELL
  return invoke<MemoryEditOutcome>("restore_agent_memory", { slug, archiveId })
}

/** Remove one dangling line from a project's MEMORY.md. */
export async function removeAgentMemoryIndexLine(
  slug: string,
  lineNumber: number,
  target: string,
): Promise<MemoryEditOutcome> {
  if (!hasShell()) return NO_SHELL
  return invoke<MemoryEditOutcome>("remove_agent_memory_index_line", {
    slug,
    lineNumber,
    target,
  })
}
