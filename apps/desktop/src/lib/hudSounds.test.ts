import { afterEach, describe, expect, it, vi } from "vitest"

import { playBwoop, playPop } from "./hudSounds"

type Fake = {
  started: number[]
  ramps: number[]
}

function installAudio(): Fake {
  const fake: Fake = { started: [], ramps: [] }
  class Gain {
    gain = {
      setValueAtTime: () => undefined,
      exponentialRampToValueAtTime: () => undefined,
    }
    connect() {
      return this
    }
  }
  class Osc {
    type = "sine"
    frequency = {
      setValueAtTime: (hz: number) => fake.started.push(hz),
      exponentialRampToValueAtTime: (hz: number) => fake.ramps.push(hz),
    }
    connect(node: unknown) {
      return node
    }
    start() {}
    stop() {}
  }
  class Context {
    state = "running"
    currentTime = 0
    destination = {}
    createOscillator() {
      return new Osc()
    }
    createGain() {
      return new Gain()
    }
    resume() {
      return Promise.resolve()
    }
  }
  vi.stubGlobal("AudioContext", Context)
  return fake
}

describe("hudSounds", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("plays nothing without an AudioContext", () => {
    vi.stubGlobal("AudioContext", undefined)
    expect(() => playPop()).not.toThrow()
    expect(() => playBwoop()).not.toThrow()
  })

  it("pops down and bwoops up", () => {
    const fake = installAudio()
    playPop()
    expect(fake.started.at(-1)!).toBeGreaterThan(fake.ramps.at(-1)!)
    playBwoop()
    expect(fake.started.at(-1)!).toBeLessThan(fake.ramps.at(-1)!)
  })
})
