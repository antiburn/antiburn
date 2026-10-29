import { discover } from './discovery.mjs'
try {
  console.log(JSON.stringify(await discover()))
} catch {
  console.log('{"state":"unavailable"}')
}
