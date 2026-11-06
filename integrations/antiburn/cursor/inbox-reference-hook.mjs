import { publishInboxReference } from './inbox-bridge.mjs'
import {
  isDirectExecution,
  runJsonHook,
} from '../../../packages/antiburn-plugin-core/native-hook.mjs'

/** Publish references separately from the existing context consumer. */
export function cursorInboxReference(input, base) {
  if (typeof base !== 'string') return
  publishInboxReference(base, input)
}

if (isDirectExecution(import.meta.filename)) {
  const args = process.argv.slice(2)
  if (args.length === 2 && args[0] === '--inbox-dir')
    await runJsonHook(async (input) => {
      cursorInboxReference(input, args[1])
    })
}
