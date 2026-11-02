import type { ClientModule } from 'claude-code'

const CheckingStatus: ClientModule<null, number> = (_props, surface) => {
  if (surface.state === undefined) {
    let phase = 0
    surface.setState(phase)
    surface.every(300, () => {
      phase = (phase + 1) % 3
      surface.setState(phase)
    })
  }
  const { Text } = surface.elements
  return Text({
    wrap: 'wrap',
    children: ['Checking usage', ...[0, 1, 2].map((index) => Text({ dimColor: index !== (surface.state ?? 0), children: '.' }))],
  })
}

export default CheckingStatus
