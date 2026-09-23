import type {
  BurnCheckDetectorId,
  BurnCheckTargetListPayload,
  ChecksCategoryPayload,
} from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checkDefinitions"

/** Checks about the agent setup. They fail in most sessions, so the chart
 *  shows each one once at the weekly reset, not as a pin per session. */
export const CONFIG_CHECKS: ReadonlySet<BurnCheckDetectorId> = new Set<BurnCheckDetectorId>([
  "overpoweredSubagents",
  "unusedMcpServers",
  "unusedBuiltInTools",
  "unusedSkills",
])

/** One wasteful session on the allowance chart. */
export interface WastePin {
  detector: BurnCheckDetectorId
  label: string
  atEpoch: number
  title: string
  navigationHandle: string
}

/** A config check and the share of checked sessions that fail it. */
export interface ConfigShare {
  detector: BurnCheckDetectorId
  label: string
  share: number
  /** The sessions that fail the check. */
  finding: number
  /** The sessions the check could read. */
  sessions: number
}

export interface WasteMarks {
  pins: WastePin[]
  config: ConfigShare[]
  onOpen?: (pin: WastePin) => void
}

/** The failing checks that get a pin per session. */
export function pinnedDetectors(
  failures: readonly ChecksCategoryPayload[],
): BurnCheckDetectorId[] {
  return failures.map((check) => check.id).filter((id) => !CONFIG_CHECKS.has(id))
}

/** The pins and config shares for the failing checks. A pin needs the check's
 *  sample sessions, so a check without loaded targets has no pins yet. */
export function wasteMarks(
  failures: readonly ChecksCategoryPayload[],
  targets: Partial<Record<BurnCheckDetectorId, { data: BurnCheckTargetListPayload | null }>>,
): Omit<WasteMarks, "onOpen"> {
  const config = failures
    .filter((check) => CONFIG_CHECKS.has(check.id) && check.finding + check.clean > 0)
    .map((check) => ({
      detector: check.id,
      label: CHECK_LABELS[check.id],
      share: check.finding / (check.finding + check.clean),
      finding: check.finding,
      sessions: check.finding + check.clean,
    }))
    .sort((left, right) => right.share - left.share)
  const pins = pinnedDetectors(failures).flatMap((detector) =>
    (targets[detector]?.data?.samples ?? []).map((sample) => ({
      detector,
      label: CHECK_LABELS[detector],
      atEpoch: sample.observedAtMs / 1000,
      title: sample.title,
      navigationHandle: sample.navigationHandle,
    })),
  )
  return { pins, config }
}
