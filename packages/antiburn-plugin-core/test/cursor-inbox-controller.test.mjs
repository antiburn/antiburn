import assert from 'node:assert/strict'
import { test } from 'node:test'
import { CursorInboxController } from '../../../integrations/antiburn/cursor/inbox-controller.mjs'
import {
  inboxFrameUpdates,
  inboxInlineMarkdown,
  renderCursorInbox,
} from '../../../integrations/antiburn/cursor/inbox-view.mjs'

/** @returns {NonNullable<Awaited<ReturnType<typeof import('../session-checks.mjs').readSessionChecks>>>} */
function saved(message = 'Keep **scope** and `code`.', receipt = 'a'.repeat(64)) {
  return {
    message: '',
    sessionScope: 'main',
    state: 'ready',
    freshnessEvaluated: false,
    receipt,
    snapshot: [
      {
        id: 'check-1',
        name: 'Authored title',
        message,
        status: 'failure',
        eligibility: 'ready',
      },
    ],
    updates: [],
  }
}
function confirmation(controller, sessionId = 'session-1') {
  return {
    frame: controller.frame,
    sessionId,
    receipt: controller.cards.get(sessionId).result.receipt,
  }
}

test('poll reads full UI snapshots separately and confirms only matching visible frames', async () => {
  const reads = [],
    acks = []
  const controller = new CursorInboxController({
    read: async (options) => {
      reads.push(options)
      return saved()
    },
    acknowledge: async (options) => {
      acks.push(options)
      return true
    },
  })
  await controller.poll(['session-1'])
  assert.equal(controller.cards.size, 1)
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  controller.setVisible(true)
  assert.equal(
    await controller.confirmApplied({ ...confirmation(controller), frame: 'old' }),
    false,
  )
  assert.equal(
    await controller.confirmApplied({ ...confirmation(controller), receipt: 'wrong' }),
    false,
  )
  assert.equal(await controller.confirmApplied(confirmation(controller)), true)
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  assert.deepEqual(reads[0], { host: 'cursor', sessionId: 'session-1', consumer: 'ui' })
  assert.equal(acks[0].consumer, 'ui')
  controller.close()
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
})

test('unrelated changes cannot re-ack an accepted receipt and unchanged snapshots do not reveal', async () => {
  const changes = [],
    acks = []
  let secondMessage = 'second'
  const controller = new CursorInboxController({
    read: async ({ sessionId }) => saved(sessionId === 'session-1' ? 'first' : secondMessage),
    acknowledge: async (options) => {
      acks.push(options)
      return true
    },
    changed: (_, options) => changes.push(options),
  })
  controller.setVisible(true)
  await controller.poll(['session-1', 'session-2'])
  assert.equal(changes.at(-1).reveal, false)
  await controller.confirmApplied(confirmation(controller))
  secondMessage = 'changed'
  await controller.poll(['session-1', 'session-2'])
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  assert.equal(acks.length, 1)
  await controller.confirmApplied(confirmation(controller, 'session-2'))
  const count = changes.length
  await controller.poll(['session-1', 'session-2'])
  assert.equal(changes.length, count)
})

test('scope, disabled state, empty message and blocked eligibility cannot show or acknowledge old warnings', async () => {
  for (const result of [
    { ...saved(), sessionScope: 'unsupported' },
    { ...saved(), state: 'disabled' },
    { ...saved(), snapshot: [{ ...saved().snapshot[0], eligibility: 'blocked' }] },
    { ...saved(), snapshot: [{ ...saved().snapshot[0], message: '' }] },
  ]) {
    const controller = new CursorInboxController({
      read: async () => result,
      acknowledge: async () => {
        throw new Error('must not acknowledge')
      },
    })
    controller.setVisible(true)
    await controller.poll(['session-1'])
    assert.equal(inboxFrameUpdates(result).length, 0)
    assert.equal(
      await controller.confirmApplied({
        frame: controller.frame,
        sessionId: 'session-1',
        receipt: result.receipt,
      }),
      false,
    )
    assert.equal(
      renderCursorInbox(controller.snapshot(), 'a'.repeat(32)).includes(
        'Keep <strong>scope</strong>',
      ),
      false,
    )
  }
})

test('changes, clearing and withdrawals render canonical transitions; closure mutes reveal', async () => {
  let result = saved()
  const reveal = []
  const controller = new CursorInboxController({
    read: async () => result,
    changed: (_, options) => reveal.push(options.reveal),
  })
  await controller.poll(['session-1'])
  assert.equal(reveal.at(-1), true)
  const old = confirmation(controller)
  controller.close()
  result = saved('Changed **message**', 'b'.repeat(64))
  await controller.poll(['session-1'])
  assert.equal(reveal.at(-1), false)
  assert.equal(await controller.confirmApplied(old), false)
  controller.resume()
  controller.setVisible(true)
  result = {
    ...saved(),
    state: 'disabled',
    snapshot: [],
    updates: [{ id: 'check-1', kind: 'invalidated', name: 'Authored title', message: null }],
  }
  await controller.poll(['session-1'])
  let html = renderCursorInbox(controller.snapshot(), 'a'.repeat(32))
  assert.ok(html.includes('withdrawn'))
  assert.ok(html.includes('────────────────────────────────'))
  result = {
    ...saved(),
    snapshot: [],
    updates: [{ id: 'check-1', kind: 'cleared', name: 'Authored title', message: null }],
  }
  await controller.poll(['session-1'])
  html = renderCursorInbox(controller.snapshot(), 'a'.repeat(32))
  assert.ok(html.includes('does not confirm a repair'))
})

test('failed reads revoke old frames, disposed polling cannot apply late results, and poll is bounded', async () => {
  /** @type {ReturnType<typeof saved>|undefined} */
  let result = saved()
  let readCount = 0
  const controller = new CursorInboxController({
    read: async () => {
      readCount++
      return result
    },
  })
  await controller.poll(['session-1'])
  const old = confirmation(controller)
  result = undefined
  await controller.poll(['session-1'])
  assert.equal(controller.cards.get('session-1').unavailable, true)
  assert.equal(await controller.confirmApplied(old), false)
  await controller.poll(Array.from({ length: 20 }, (_, i) => `session-${i}`))
  assert.equal(readCount, 6)
  let complete
  const late = new CursorInboxController({
    read: () =>
      new Promise((resolve) => {
        complete = resolve
      }),
  })
  const pending = late.poll(['session-1'])
  await late.poll(['session-1'])
  late.dispose()
  complete(saved())
  await pending
  assert.equal(late.cards.size, 0)
})

test('allowlisted inline Markdown never executes authored HTML, resources or links', () => {
  assert.equal(inboxInlineMarkdown('['.repeat(16_384)), '['.repeat(16_384))
  const text =
    '**bold** *emphasis* `code` [site](https://example.com) <script>bad()</script> <img src=x>\nEND'
  const rendered = inboxInlineMarkdown(text)
  assert.ok(rendered.includes('<strong>bold</strong>'))
  assert.ok(rendered.includes('<em>emphasis</em>'))
  assert.ok(rendered.includes('<code>code</code>'))
  assert.ok(rendered.includes('site (https://example.com)'))
  assert.ok(rendered.includes('&lt;script&gt;'))
  assert.ok(!/<script|<img|<a\b/.test(rendered))
  assert.ok(rendered.endsWith('\nEND'))
  const html = renderCursorInbox(
    { frame: 'a'.repeat(32), cards: [{ sessionId: 'session-1', result: saved(text) }] },
    'b'.repeat(32),
  )
  assert.equal((html.match(/<script /g) || []).length, 1)
  assert.ok(html.includes("default-src 'none'"))
  assert.ok(html.includes('<title>Antiburn Inbox</title>'))
  assert.ok(html.includes('<h1>antiburn Inbox</h1>'))
  assert.ok(html.includes('🔥 <code>antiburn</code> · <strong>1 finding</strong>'))
  assert.ok(!html.includes('<details>'))
  assert.ok(html.includes('END'))
})

test('escaped check titles keep literal punctuation inside their bold heading', () => {
  const result = saved()
  result.snapshot[0].name = 'my_check [literal] *stars* `code` \\ <img src=x>'
  const html = renderCursorInbox(
    { frame: 'a'.repeat(32), cards: [{ sessionId: 'session-1', result }] },
    'b'.repeat(32),
  )
  assert.ok(
    html.includes('<strong>my_check [literal] *stars* `code` \\ &lt;img src=x&gt;</strong>'),
  )
  assert.ok(!html.includes('<img'))
  assert.equal(inboxInlineMarkdown('\\*literal\\* \\[label\\]'), '*literal* [label]')
  assert.equal(inboxInlineMarkdown('`a\\_b`'), '<code>a\\_b</code>')
})

test('acknowledgment failure remains pending and revocation before confirmation prevents writes', async () => {
  let accept = false
  let calls = 0
  const controller = new CursorInboxController({
    read: async () => saved(),
    acknowledge: async () => {
      calls++
      if (!accept) throw new Error('write failed')
      return true
    },
  })
  await controller.poll(['session-1'])
  controller.setVisible(true)
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  accept = true
  assert.equal(await controller.confirmApplied(confirmation(controller)), true)
  controller.setVisible(false)
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  controller.dispose()
  assert.equal(
    await controller.confirmApplied({
      frame: controller.frame,
      sessionId: 'session-1',
      receipt: 'a'.repeat(64),
    }),
    false,
  )
  assert.equal(calls, 2)
})

test('unchanged visible polls retry failed receipts through a fresh DOM frame and stop after acceptance', async () => {
  for (const failure of ['false', 'throw']) {
    let calls = 0
    const renders = []
    const controller = new CursorInboxController({
      read: async () => saved(),
      changed: (snapshot, options) => renders.push({ snapshot, options }),
      acknowledge: async () => {
        calls++
        if (calls > 1) return true
        if (failure === 'throw') throw new Error('transient failure')
        return false
      },
    })
    controller.setVisible(true)
    await controller.poll(['session-1'])
    const stale = confirmation(controller)
    assert.equal(await controller.confirmApplied(stale), false)
    await controller.poll(['session-1'])
    assert.equal(calls, 1)
    assert.equal(renders.length, 2)
    assert.equal(renders[1].options.reveal, false)
    assert.notEqual(controller.frame, stale.frame)
    assert.equal(await controller.confirmApplied(stale), false)
    assert.equal(await controller.confirmApplied(confirmation(controller)), true)
    await controller.poll(['session-1'])
    assert.equal(calls, 2)
    assert.equal(renders.length, 2)
  }
})

test('unchanged hidden or muted views do not request confirmation or acknowledge pending receipts', async () => {
  let calls = 0
  let renders = 0
  const controller = new CursorInboxController({
    read: async () => saved(),
    changed: () => {
      renders++
    },
    acknowledge: async () => {
      calls++
      return false
    },
  })
  controller.setVisible(true)
  await controller.poll(['session-1'])
  await controller.confirmApplied(confirmation(controller))
  controller.setVisible(false)
  await controller.poll(['session-1'])
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  controller.close()
  await controller.poll(['session-1'])
  assert.equal(await controller.confirmApplied(confirmation(controller)), false)
  assert.equal(renders, 1)
  assert.equal(calls, 1)
})

test('a pending acknowledgement does not trigger another unchanged render', async () => {
  let complete
  let renders = 0
  const controller = new CursorInboxController({
    read: async () => saved(),
    changed: () => {
      renders++
    },
    acknowledge: () =>
      new Promise((resolve) => {
        complete = resolve
      }),
  })
  controller.setVisible(true)
  await controller.poll(['session-1'])
  const frame = controller.frame
  const acknowledging = controller.confirmApplied(confirmation(controller))
  await controller.poll(['session-1'])
  assert.equal(controller.frame, frame)
  assert.equal(renders, 1)
  complete(true)
  await acknowledging
})
