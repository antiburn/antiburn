import { expect, test } from 'claude-code/testing'
import { liveMode } from './mode'

test('usage is the default and only an exact checks flag enables checks', () => {
  for (const flag of [undefined, '', 'usage', 'invalid', 'CHECKS', ' checks']) expect(liveMode(flag)).toBe('usage')
  expect(liveMode('checks')).toBe('checks')
})
