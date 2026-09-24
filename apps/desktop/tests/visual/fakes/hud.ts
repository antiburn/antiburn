import type { HudIslandState } from "../../../src/lib/hudIsland"
import type { HudDetailMap, HudTokenMapPayload } from "../../../src/lib/hudIpc"

export function fixtureIsland(
  params = new URLSearchParams(window.location.search),
): HudIslandState {
  const phase = params.get("island")
  const island =
    phase === "collapsed" || phase === "expanded" || phase === "preview" ? phase : "off"
  const scale = (Number(params.get("scale")) || 100) / 100
  const wing = 30 * scale
  const headerWidth = 204 + 2 * wing + 38
  const width = island === "expanded" ? Math.max(headerWidth, 264 * scale + 38) : headerWidth
  return {
    island,
    scale,
    revision: 7,
    wing,
    fillet: 19,
    notch: 204,
    height: params.get("header") === "short" ? 16 : 32,
    bodyWidth: width - 38,
    bodyMaxHeight: params.get("display") === "short" ? 170 : 500,
    headerOffset: (width - headerWidth) / 2,
  }
}

export function fixtureTokenMap(): HudTokenMapPayload | null {
  const params = new URLSearchParams(window.location.search)
  if (params.get("map") !== "on" || params.get("state") === "empty") return null
  return {
    nowEpoch: Date.parse("2026-09-15T00:00:00.000Z") / 1000,
    windowSecs: 300,
    spend: { usdPerMinute: 0.12, windowSecs: 300, pricedShare: 1 },
    sessions: Array.from({ length: params.get("state") === "long" ? 8 : 3 }, (_, index) => ({
      agent: "codex",
      sessionId: `fixture-map-${index}`,
      title: `Session ${index + 1}: inspect the interface scaling contract`,
      lastTurnEpoch: Date.parse("2026-09-15T00:00:00.000Z") / 1000,
      tokensPerMin: 1200 - index * 100,
      modes: {
        looking: 100,
        running: 200,
        changing: 500,
        delegating: 100,
        thinking: 200,
        talking: 100,
        other: 0,
      },
      subagents:
        index === 0
          ? [
              {
                subagentId: "fixture-worker",
                tokensPerMin: 250,
                modes: {
                  looking: 150,
                  running: 100,
                  changing: 0,
                  delegating: 0,
                  thinking: 0,
                  talking: 0,
                  other: 0,
                },
              },
            ]
          : [],
    })),
  }
}

export function fixtureDetailMap(): HudDetailMap | null {
  const payload = fixtureTokenMap()
  return payload
    ? {
        dotValue: 100,
        sessions: payload.sessions.map((session) => ({
          ...session,
          key: session.sessionId,
          label: session.title ?? session.agent,
          topMode: "changing",
          frameColor: "var(--color-brand-tint)",
        })),
      }
    : null
}
