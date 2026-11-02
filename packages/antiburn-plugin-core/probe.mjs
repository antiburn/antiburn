import { discover } from './discovery.mjs'

const result = await discover().catch(() => ({ state: 'unavailable' }))
process.stdout.write(`${JSON.stringify(result)}\n`)
