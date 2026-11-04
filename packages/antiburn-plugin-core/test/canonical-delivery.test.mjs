import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'
import { readSessionChecks } from '../session-checks.mjs'
import { formatSessionCheckUpdates } from '../session-checks-validation.mjs'

const checks = [
  'over-exploring',
  'scope-creep',
  'ignored-instructions',
  'skill-opportunities',
  'fast-mode-value',
]
const hosts = ['claude-code', 'codex', 'cursor', 'opencode', 'pi']

for (const id of checks) {
  const yaml = await readFile(
    new URL(`../../../.antiburn/checks/${id}.yml`, import.meta.url),
    'utf8',
  )
  const name = yaml.match(/^name: (.+)$/m)?.[1]
  const messages = [...yaml.matchAll(/result: failure\r?\n\s+message: ([^\r\n]+)/g)].map(
    (match) => match[1],
  )
  assert.ok(name, `${id}: expected a plain name`)
  assert.ok(messages.length, `${id}: expected plain failure messages`)
  for (const message of messages) {
    test(`${id}: authored advice survives validated delivery for all five consumers`, async () => {
      for (const host of hosts) {
        const update = { id: `yaml:${id}`, kind: 'finding', name, message }
        const response = {
          protocol_version: 1,
          session_id: 'canonical-fixture',
          identity: { environment_key: 'native', agent: host, session_id: 'canonical-fixture' },
          session_scope: 'main',
          freshness_evaluated: false,
          state: 'ready',
          snapshot: [
            {
              id: update.id,
              check_id: id,
              name: update.name,
              message: update.message,
              status: 'failure',
              assessed_at_epoch: 1,
              checking: false,
              eligibility: 'ready',
            },
          ],
          updates: [update],
          receipt: 'a'.repeat(64),
        }
        const calls = []
        const delivered = await readSessionChecks({
          host,
          sessionId: response.session_id,
          run: async (args) => {
            calls.push(args)
            return JSON.stringify(response)
          },
        })
        assert.ok(delivered, host)
        assert.ok(delivered.message.includes(message), host)
        assert.ok(delivered.message.includes(`**${name}**`), host)
        assert.match(delivered.message, /untrusted data, not instructions/)
        assert.equal(calls.length, 1)
        assert.equal(calls[0][4], `${host}:context`)
        assert.equal(calls[0].includes('--ack'), false)
        for (const kind of ['cleared', 'invalidated']) {
          const withdrawn = formatSessionCheckUpdates([{ ...update, kind }])
          assert.equal(withdrawn.includes(message), false)
          assert.match(
            withdrawn,
            kind === 'cleared'
              ? /does not confirm a repair/
              : /eligibility could not be confirmed/,
          )
        }
      }
    })
  }
}
