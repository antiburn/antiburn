import { randomBytes } from 'node:crypto'
import {
  readSessionChecks,
  acknowledgeSessionChecks,
} from '../../../packages/antiburn-plugin-core/session-checks.mjs'
import { inboxFrameUpdates } from './inbox-view.mjs'

/** Coordinate full snapshots and visible application receipts for one enrolled Inbox. */
export class CursorInboxController {
  constructor({
    read = readSessionChecks,
    acknowledge = acknowledgeSessionChecks,
    changed = (_snapshot, _options) => {},
  } = {}) {
    this.read = read
    this.acknowledge = acknowledge
    this.changed = changed
    this.cards = new Map()
    this.visible = false
    this.muted = false
    this.disposed = false
    this.polling = false
    this.offset = 0
    this.frame = randomBytes(16).toString('hex')
    this.acknowledging = new Set()
    this.acceptedReceipts = new Map()
  }

  invalidateFrame() {
    this.frame = randomBytes(16).toString('hex')
  }

  setVisible(visible) {
    if (this.disposed) return
    if (this.visible !== visible) this.invalidateFrame()
    this.visible = visible
  }

  close() {
    this.visible = false
    this.muted = true
    this.invalidateFrame()
  }

  resume() {
    if (this.disposed) return
    this.muted = false
    this.invalidateFrame()
  }

  dispose() {
    this.disposed = true
    this.close()
    this.cards.clear()
    this.acceptedReceipts.clear()
  }

  snapshot() {
    return { frame: this.frame, cards: [...this.cards.values()] }
  }

  async poll(sessionIds) {
    if (this.disposed || this.polling) return
    this.polling = true
    try {
      const sessions = [...new Set(sessionIds)]
        .filter((id) => typeof id === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(id))
        .slice(0, 16)
      let changed = false
      let reveal = false
      for (const key of this.cards.keys()) {
        if (!sessions.includes(key)) {
          this.cards.delete(key)
          this.acceptedReceipts.delete(key)
          changed = true
        }
      }
      const batch = Array.from(
        { length: Math.min(4, sessions.length) },
        (_, index) => sessions[(this.offset + index) % sessions.length],
      )
      this.offset = sessions.length ? (this.offset + batch.length) % sessions.length : 0
      for (const sessionId of batch) {
        let result
        try {
          result = await this.read({ host: 'cursor', sessionId, consumer: 'ui' })
        } catch {
          result = undefined
        }
        if (this.disposed) return
        const card =
          result?.sessionScope === 'main'
            ? { sessionId, result }
            : { sessionId, unavailable: true }
        const previous = this.cards.get(sessionId)
        if (JSON.stringify(card) !== JSON.stringify(previous)) {
          this.cards.set(sessionId, card)
          changed = true
          const failures = card.result
            ? inboxFrameUpdates(card.result).filter((row) => row.kind === 'finding')
            : []
          const previousFailures = previous?.result
            ? inboxFrameUpdates(previous.result).filter((row) => row.kind === 'finding')
            : []
          if (failures.length && JSON.stringify(failures) !== JSON.stringify(previousFailures))
            reveal = true
        }
      }
      const needsConfirmation =
        this.visible &&
        !this.muted &&
        [...this.cards.values()].some(
          (card) =>
            card.result?.receipt &&
            inboxFrameUpdates(card.result).length &&
            this.acceptedReceipts.get(card.sessionId) !== card.result.receipt &&
            !this.acknowledging.has(card.sessionId),
        )
      if (changed || needsConfirmation) {
        this.invalidateFrame()
        this.changed(this.snapshot(), { reveal: reveal && !this.visible && !this.muted })
      }
    } finally {
      this.polling = false
    }
  }

  /** Call only after the correct visible DOM applies this exact frame. */
  async confirmApplied(message) {
    if (
      this.disposed ||
      this.muted ||
      !this.visible ||
      message?.frame !== this.frame ||
      typeof message.sessionId !== 'string'
    )
      return false
    const card = this.cards.get(message.sessionId)
    const receipt = card?.result?.receipt
    if (
      !receipt ||
      !inboxFrameUpdates(card.result).length ||
      message.receipt !== receipt ||
      this.acceptedReceipts.get(message.sessionId) === receipt ||
      this.acknowledging.has(message.sessionId)
    )
      return false
    const frame = this.frame
    const sessionId = message.sessionId
    this.acknowledging.add(sessionId)
    try {
      if (
        !this.visible ||
        this.muted ||
        this.disposed ||
        frame !== this.frame ||
        this.cards.get(sessionId) !== card
      )
        return false
      const accepted = await this.acknowledge({
        host: 'cursor',
        sessionId,
        consumer: 'ui',
        receipt,
      })
      if (accepted && !this.disposed && this.cards.has(sessionId))
        this.acceptedReceipts.set(sessionId, receipt)
      return accepted
    } catch {
      return false
    } finally {
      this.acknowledging.delete(sessionId)
    }
  }
}
