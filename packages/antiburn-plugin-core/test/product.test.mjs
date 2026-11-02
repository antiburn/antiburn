import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  describe,
  parseCommand,
  renderHelp,
  renderStatus,
  parseUsageCommand,
  renderUsageHelp,
} from '../product.mjs'

test('product parser accepts supported commands and presentation flags', () => {
  for (const command of ['help', 'status', 'diagnostics', 'usage', 'details', 'refresh'])
    assert.equal(parseCommand(command).command, command)
  assert.equal(parseCommand('').command, 'help')
  assert.deepEqual(parseCommand('status --ascii'), { command: 'status', style: 'ascii' })
  assert.deepEqual(parseCommand('help --flame'), { command: 'help', style: 'flame' })
  assert.equal(parseCommand('usage details').command, 'details')
  assert.equal(parseCommand('usage refresh').command, 'refresh')
})

test('product parser rejects unsupported arguments and flags', () => {
  for (const input of [
    'help extra',
    'status --ascii extra',
    'usage --ascii',
    'diagnostics refresh',
    'checks',
    'x'.repeat(513),
  ])
    assert.equal(parseCommand(input).command, 'invalid')
})

test('public product help and status describe usage without CLI integration', () => {
  const meta = describe('claude-code', '/antiburn', 'native', 'desktop', '2.1.302')
  assert.equal(meta.capabilities.usage, true)
  assert.match(
    renderHelp(meta),
    /Usage: \/antiburn \[help\|status\|diagnostics\|usage \[details\|refresh\]\]/,
  )
  assert.match(renderHelp(meta), /Help and status accept --ascii or --flame/)
  assert.match(renderHelp(meta), /Aliases: details and refresh show or refresh usage/)
  assert.doesNotMatch(renderHelp(meta), /Usage:.*--ascii/)
  assert.doesNotMatch(renderHelp(meta), /CLI|executable|App connection/)
  assert.match(renderStatus(meta), /Usage feature: Included/)
  assert.match(
    renderStatus(describe('codex', 'help', 'model-mediated')),
    /Usage feature: Not included/,
  )
  assert.doesNotMatch(renderStatus(meta), /CLI|App connection/)
})

test('public usage commands keep defaults and help simple', () => {
  for (const args of ['', 'usage', 'details', 'usage details'])
    assert.equal(parseUsageCommand(args), 'details')
  for (const args of ['refresh', 'usage refresh', '  refresh  '])
    assert.equal(parseUsageCommand(args), 'refresh')
  assert.equal(parseUsageCommand('help'), 'help')
  for (const args of [
    'status',
    'diagnostics',
    'help --ascii',
    'help --flame',
    'refresh extra',
    'x'.repeat(513),
  ])
    assert.equal(parseUsageCommand(args), 'invalid')
  assert.equal(
    renderUsageHelp('/antiburn'),
    'Run `/antiburn` to see your usage.\nRun `/antiburn refresh` to update it.',
  )
  assert.match(renderUsageHelp('/antiburn', true), /^Unknown arguments\./)
  assert.doesNotMatch(
    renderUsageHelp('/antiburn'),
    /contract|version|Host:|antiburn:|🔥|--ascii|--flame/,
  )
})
