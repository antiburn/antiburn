import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

/** One Claude Code configuration directory, as Settings shows it. */
export interface ClaudeProfile {
  id: string
  label: string
  path: string
  /** The CLI's default directory. It can be renamed but not removed. */
  builtIn: boolean
}

/** A `~/.claude*` directory that holds sessions and is not a profile yet. */
export interface ClaudeProfileSuggestion {
  path: string
  label: string
}

export interface ClaudeProfilesPayload {
  profiles: ClaudeProfile[]
  suggestions: ClaudeProfileSuggestion[]
  maxProfiles: number
  /** The longest name the shell accepts, in characters. */
  maxLabelChars: number
}

export const EMPTY_CLAUDE_PROFILES: ClaudeProfilesPayload = {
  profiles: [],
  suggestions: [],
  maxProfiles: 0,
  maxLabelChars: 0,
}

export async function listClaudeProfiles(): Promise<ClaudeProfilesPayload> {
  if (!hasShell()) return EMPTY_CLAUDE_PROFILES
  return invoke<ClaudeProfilesPayload>("list_claude_profiles")
}

export async function addClaudeProfile(
  label: string,
  path: string,
): Promise<ClaudeProfilesPayload> {
  return invoke<ClaudeProfilesPayload>("add_claude_profile", { label, path })
}

export async function renameClaudeProfile(
  id: string,
  label: string,
): Promise<ClaudeProfilesPayload> {
  return invoke<ClaudeProfilesPayload>("rename_claude_profile", { id, label })
}

export async function removeClaudeProfile(id: string): Promise<ClaudeProfilesPayload> {
  return invoke<ClaudeProfilesPayload>("remove_claude_profile", { id })
}

/** The number of profiles the reader added, without the built-in one. */
export function addedProfileCount(payload: ClaudeProfilesPayload): number {
  return payload.profiles.filter((profile) => !profile.builtIn).length
}
