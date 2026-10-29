import { expect, test } from 'claude-code/testing'

import { LiveChecks, type CheckId, type LiveView } from './checks'
import { TOUR } from './tour'

function stages(view: LiveView): Partial<Record<CheckId, string>> {
  return Object.fromEntries(view.checks.map(check => [check.id, check.stage]))
}

function play(checks: LiveChecks, index: number): LiveView {
  const beat = TOUR[index]
  if (beat === undefined) throw new Error(`no tour step ${index}`)
  beat.run(checks)
  checks.turnEnd(undefined)
  return checks.takeView()
}

test('the tour adds checks step by step through the real gates', () => {
  const checks = new LiveChecks()
  expect(TOUR.length).toBe(4)

  const one = play(checks, 0)
  expect(stages(one)).toEqual({ X1: 'active', T: 'building' })
  expect(one.newIds).toEqual(['X1'])

  const two = play(checks, 1)
  expect(TOUR[1]?.dismissesStrip).toBe(true)
  expect(stages(two)).toEqual({ X1: 'active', T: 'active', E1: 'active', K2: 'active' })
  expect([...two.newIds].sort()).toEqual(['E1', 'K2', 'T'])

  const three = play(checks, 2)
  expect(stages(three)).toMatchObject({ X1: 'resolved', D: 'active', E1: 'active' })
  expect(three.newIds).toEqual(['D'])
  expect(three.resolvedNow).toEqual(['X1'])
  expect(three.checks.find(check => check.id === 'D')?.cause).toMatch(/^File reads are about \d+% of it/)

  const four = play(checks, 3)
  expect(stages(four)).toMatchObject({ D: 'active', T: 'active', K2: 'active', K1: 'active', C1: 'active' })
  expect([...four.newIds].sort()).toEqual(['C1', 'K1'])
  expect(four.checks.find(check => check.id === 'K1')?.metric).toBe('/release-notes fits · 12 steps')
  expect(four.checks.find(check => check.id === 'C1')?.metric).toBe('3 files outside the plan')
})

test('a fix pressed during the tour resolves its check as fixed on the next step', () => {
  const checks = new LiveChecks()
  for (let index = 0; index < 2; index += 1) play(checks, index)
  checks.fixSent('X1')
  expect(checks.takeView().checks.find(check => check.id === 'X1')?.stage).toBe('watching')
  const three = play(checks, 2)
  const x1 = three.checks.find(check => check.id === 'X1')
  expect(x1?.stage).toBe('resolved')
  expect(x1?.isStoppedAfterFix).toBe(true)
})
