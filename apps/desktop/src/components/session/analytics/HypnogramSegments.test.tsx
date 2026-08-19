// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { PhaseSegment, SessionPhase, SkillDetail } from "../../../lib/types/session"
import { HypnogramSegments, type MarkerLayer } from "./HypnogramSegments"
import { coalesce } from "./HypnogramSegments.logic"
import type { TimelineMarker } from "./markerCluster"
import { skillMarkerKind } from "./skillMarkerKind"
import { spawnMarkerKind, type SpawnPayload } from "./spawnMarkerKind"

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

/** Force the chart's measured width so size-aware clustering can be tested. */
function stubChartWidth(width: number): void {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    width,
    height: 20,
    top: 0,
    left: 0,
    right: width,
    bottom: 20,
    x: 0,
    y: 0,
    toJSON: () => {},
  } as DOMRect)
}

function seg(
  phase: SessionPhase,
  activeMs: number,
  over: Partial<PhaseSegment> = {},
): PhaseSegment {
  return { phase, activeMs, tokensIn: 0, tokensOut: 0, contextTokens: 0, ...over }
}

const totalMs = (segments: PhaseSegment[]) => segments.reduce((s, x) => s + x.activeMs, 0)

/** A single full-width segment makes the active-time axis linear. */
const ONE_SEGMENT = [seg("implementing", 1000)]

function marker(
  progress: number,
  label: string,
  open?: () => void,
): TimelineMarker<SpawnPayload> {
  return {
    progress,
    data: { patternScore: 80, agent: "claude-code", label, open: open ?? (() => {}) },
  }
}

function spawnLayer(markers: TimelineMarker<SpawnPayload>[]): MarkerLayer[] {
  return [{ markers, kind: spawnMarkerKind } as MarkerLayer]
}

function skillMarker(
  progress: number,
  over: Partial<SkillDetail> = {},
): TimelineMarker<SkillDetail> {
  return {
    progress,
    data: { name: "deep-research", progress, tokensOut: 0, contextTokens: 0, ...over },
  }
}

function skillLayer(markers: TimelineMarker<SkillDetail>[]): MarkerLayer[] {
  return [{ markers, kind: skillMarkerKind } as MarkerLayer]
}

/** The `left` style of a positioned cluster dot. */
function leftOf(dot: HTMLElement): string {
  return dot.style.left
}

describe("HypnogramSegments", () => {
  it("draws a labelled ribbon with no markers by default", () => {
    render(<HypnogramSegments segments={ONE_SEGMENT} activeSecs={100} contextWindow={200000} />)
    expect(screen.getByRole("img", { name: "Session mode over time" })).toBeTruthy()
    expect(screen.queryAllByLabelText(/spawned/i)).toHaveLength(0)
  })

  it("renders nothing but the frame for an empty session", () => {
    render(<HypnogramSegments segments={[]} activeSecs={0} contextWindow={0} />)
    expect(screen.getByRole("img", { name: "Session mode over time" })).toBeTruthy()
  })
})

describe("HypnogramSegments spawn dots", () => {
  it("keeps well-separated markers as individual dots, positioned on the axis", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={spawnLayer([marker(0, "First"), marker(0.5, "Middle")])}
      />,
    )
    expect(screen.getAllByLabelText(/sub-agent spawned/i)).toHaveLength(2)
    expect(leftOf(screen.getByLabelText("Sub-agent spawned: First"))).toBe("0%")
    expect(leftOf(screen.getByLabelText("Sub-agent spawned: Middle"))).toBe("50%")
  })

  it("merges near-simultaneous spawns into one dot showing the count", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={spawnLayer([marker(0.5, "Alpha"), marker(0.51, "Beta")])}
      />,
    )
    const cluster = screen.getByLabelText("2 sub-agents spawned")
    expect(cluster.textContent).toBe("2")
    // Positioned at the mean of the two member positions.
    expect(leftOf(cluster)).toBe("50.5%")

    expect(screen.queryByText("Alpha")).toBeNull()
    fireEvent.mouseEnter(cluster)
    expect(screen.getByText("Alpha")).toBeTruthy()
    expect(screen.getByText("Beta")).toBeTruthy()
  })

  it("merges dots whose size-scaled circles would collide once the width is known", () => {
    stubChartWidth(100)
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={spawnLayer([marker(0.5, "Alpha"), marker(0.55, "Beta")])}
      />,
    )
    expect(screen.getByLabelText("2 sub-agents spawned")).toBeTruthy()
    expect(screen.queryByLabelText(/Sub-agent spawned:/)).toBeNull()
  })

  it("opens the sub-agent when a lone dot is clicked", () => {
    const onClick = vi.fn()
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={spawnLayer([marker(0.25, "Refactor", onClick)])}
      />,
    )
    fireEvent.click(screen.getByLabelText("Sub-agent spawned: Refactor"))
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("opens a sub-agent from its row inside a merged cluster's card", () => {
    const onAlpha = vi.fn()
    const onBeta = vi.fn()
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={spawnLayer([marker(0.5, "Alpha", onAlpha), marker(0.51, "Beta", onBeta)])}
      />,
    )
    fireEvent.mouseEnter(screen.getByLabelText("2 sub-agents spawned"))
    fireEvent.click(screen.getByText("Beta"))
    expect(onBeta).toHaveBeenCalledTimes(1)
    expect(onAlpha).not.toHaveBeenCalled()
  })
})

describe("HypnogramSegments skill dots", () => {
  it("renders a skill dot and shows its details on hover", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={skillLayer([
          skillMarker(0.5, {
            description: "Transcript one-liner.",
            tokensOut: 1234,
            local: {
              version: "2.0.1",
              description: "Authoritative SKILL.md description.",
              allowedTools: ["Read", "Edit"],
              scope: "global",
              available: true,
            },
          }),
        ])}
      />,
    )
    const dot = screen.getByLabelText("Skill: deep-research")
    expect(leftOf(dot)).toBe("50%")

    expect(screen.queryByText("deep-research")).toBeNull()
    fireEvent.mouseEnter(dot)
    expect(screen.getByText("deep-research")).toBeTruthy()
    expect(screen.getByText("Authoritative SKILL.md description.")).toBeTruthy()
    expect(screen.queryByText(/v2\.0\.1/)).toBeNull()

    fireEvent.click(screen.getByText("More details"))
    expect(screen.getByText(/v2\.0\.1 · Global · 2 tools/)).toBeTruthy()
  })

  it("merges recurring same-name skills into one dot with a count", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={skillLayer([skillMarker(0.5), skillMarker(0.51)])}
      />,
    )
    const dot = screen.getByLabelText("2 skills used")
    expect(dot.textContent).toBe("2")
    fireEvent.mouseEnter(dot)
    expect(screen.getByText(/×2/)).toBeTruthy()
  })

  it("stacks spawn and skill layers without merging across species", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={[
          { markers: [marker(0.5, "Sub")], kind: spawnMarkerKind } as MarkerLayer,
          { markers: [skillMarker(0.5)], kind: skillMarkerKind } as MarkerLayer,
        ]}
      />,
    )
    expect(screen.getByLabelText("Sub-agent spawned: Sub")).toBeTruthy()
    expect(screen.getByLabelText("Skill: deep-research")).toBeTruthy()
  })

  it("shows only one hover card at a time across layers", () => {
    render(
      <HypnogramSegments
        segments={ONE_SEGMENT}
        activeSecs={100}
        contextWindow={200000}
        layers={[
          { markers: [marker(0.5, "Sub")], kind: spawnMarkerKind } as MarkerLayer,
          { markers: [skillMarker(0.5)], kind: skillMarkerKind } as MarkerLayer,
        ]}
      />,
    )
    fireEvent.mouseEnter(screen.getByLabelText("Sub-agent spawned: Sub"))
    expect(screen.getByText("Sub")).toBeTruthy()

    fireEvent.mouseEnter(screen.getByLabelText("Skill: deep-research"))
    expect(screen.getByText("deep-research")).toBeTruthy()
    expect(screen.queryByText("Sub")).toBeNull()
  })
})

describe("coalesce", () => {
  it("keeps a phase's sole segment even when it is below the despeckle floor", () => {
    // The error band is well under the floor, but it is the only one, so it
    // must survive — otherwise the ribbon disagrees with the mode breakdown.
    const out = coalesce([
      seg("implementing", 100_000),
      seg("disruption", 1_000),
      seg("exploring", 100_000),
    ])
    expect(out.some((s) => s.phase === "disruption")).toBe(true)
  })

  it("still despeckles tiny segments when the phase has other survivors", () => {
    const input = [
      seg("exploring", 100_000),
      seg("implementing", 200),
      seg("exploring", 100_000),
      seg("implementing", 200),
      seg("exploring", 100_000),
      seg("implementing", 100_000),
    ]
    const out = coalesce(input)
    expect(out.length).toBeLessThan(input.length)
    expect(out.filter((s) => s.phase === "implementing")).toHaveLength(1)
  })

  it("leaves a calm session untouched", () => {
    const input = [
      seg("implementing", 60_000),
      seg("testing", 45_000),
      seg("exploring", 50_000),
      seg("thinking", 40_000),
    ]
    const out = coalesce(input)
    expect(out.map((s) => s.phase)).toEqual([
      "implementing",
      "testing",
      "exploring",
      "thinking",
    ])
    expect(out.map((s) => s.activeMs)).toEqual([60_000, 45_000, 50_000, 40_000])
  })

  it("merges adjacent same-phase runs", () => {
    const out = coalesce([seg("implementing", 10_000), seg("implementing", 5_000)])
    expect(out).toHaveLength(1)
    expect(out[0]?.activeMs).toBe(15_000)
  })

  it("conserves total active time", () => {
    const input = [
      seg("implementing", 100_000),
      seg("disruption", 1_000),
      seg("exploring", 80_000),
      seg("thinking", 300),
      seg("implementing", 90_000),
    ]
    expect(totalMs(coalesce(input))).toBe(totalMs(input))
  })

  it("does not mutate the input segments", () => {
    const input = [seg("implementing", 10_000), seg("implementing", 5_000)]
    coalesce(input)
    expect(input.map((s) => s.activeMs)).toEqual([10_000, 5_000])
  })
})
