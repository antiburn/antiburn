import type { ClientSurface, Color } from 'claude-code'

export type UsageTargetProps = { binding: string; width: number; text: string; color?: Color; dimColor?: boolean }
export type UsageTargetState = { binding: string; hovered: boolean; pressed: boolean }

export function wireUsageTarget(props: UsageTargetProps, surface: ClientSurface<UsageTargetState>): void {
  surface.onPointer((event) => {
    const previous = surface.state?.binding === props.binding ? surface.state : { binding: props.binding, hovered: false, pressed: false }
    const hovered = event.type !== 'leave' && event.y === 0 && event.x >= 0 && event.x < props.width
    const pressed = event.type === 'down' ? hovered && event.button === 'left' : event.type === 'up' ? false : previous.pressed
    const activate = event.type === 'up' && event.button === 'left' && previous.pressed && hovered
    if (hovered !== previous.hovered || pressed !== previous.pressed) surface.setState({ binding: props.binding, hovered, pressed })
    if (hovered !== previous.hovered || activate) surface.post({ binding: props.binding, hovered, activate })
  })
  surface.onKey((event) => {
    if (event.key === 'return' && !event.ctrl && !event.meta && !event.shift) {
      surface.post({ binding: props.binding, hovered: surface.state?.binding === props.binding && surface.state.hovered, activate: true })
    }
  })
}

export default function UsageTarget(props: UsageTargetProps, surface: ClientSurface<UsageTargetState>) {
  wireUsageTarget(props, surface)
  const { Text } = surface.elements
  return (
    <Text color={props.color} dimColor={props.dimColor} wrap="truncate">
      {props.text}
    </Text>
  )
}
