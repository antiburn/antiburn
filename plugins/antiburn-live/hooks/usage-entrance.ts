import type { RenderElement, RenderNode } from 'claude-code'

export const USAGE_ENTRANCE = {
  duration: 240,
  reducedDuration: 80,
  easing: 'cubic-bezier(.23,1,.32,1)',
} as const

export function fadeUsageSvg(source: string, elapsed: number, identity = '',
): string {
  if (!Number.isFinite(elapsed) || elapsed >= USAGE_ENTRANCE.duration) return source
  const animation = identity
    ? `usage-enter-${identity.replace(/[^a-zA-Z0-9_-]/g, '-')}`
    : "usage-enter"
  const delay = -Math.max(0, elapsed)
  const open = source.indexOf(">") + 1
  return (
    source.slice(0, open) +
    `<style>.usage-entrance{animation:${animation} ${USAGE_ENTRANCE.duration}ms ${USAGE_ENTRANCE.easing} ${delay}ms both}@keyframes ${animation}{from{opacity:0}to{opacity:1}}@media(prefers-reduced-motion:reduce){.usage-entrance{animation-duration:${USAGE_ENTRANCE.reducedDuration}ms}}</style><g class="usage-entrance" data-usage-entrance="${identity}">` +
    source.slice(open).replace(/<\/svg>\s*$/, "</g></svg>")
  )
}

type Graphic = { original: string; source: string }
type Site = { startedAt: number; graphics: Map<string, Graphic> }

// Each module load owns its entrances. Persisted session state cannot suppress a reload.
export class UsageEntrance {
  generation = 0
  private readonly identity = Math.random().toString(36).slice(2)
  private sites = new Map<string, Site>()
  private clients = new Set<string>()
  private detached = false

  start(): void {
    this.clients.clear()
    this.detached = false
    this.reset()
  }

  attach(clientId: string): void {
    if (this.clients.has(clientId)) return
    // The SDK shares one desktop drawing and does not identify its client.
    if (this.detached && this.clients.size === 0) this.reset()
    this.clients.add(clientId)
    this.detached = false
  }

  detach(clientId: string): void {
    this.clients.delete(clientId)
    if (this.clients.size === 0) this.detached = true
  }

  reset(): void {
    this.generation++
    this.sites.clear()
  }

  render<T extends RenderNode>(drawing: T, requestId: string, now: number): T {
    if (!Number.isFinite(now)) return drawing
    let site = this.sites.get(requestId)
    const graphics = new Map<string, Graphic>()
    const visit = (node: RenderNode, path: string): RenderNode => {
      if (typeof node === "string") return node
      if (node.type === "Svg") {
        if (site === undefined) {
          if (this.sites.size >= 8) return node
          site = { startedAt: now, graphics: new Map() }
          this.sites.set(requestId, site)
        }
        const elapsed = Math.max(0, now - site.startedAt)
        const previous = site.graphics
        const original = node.props.source
        const cached = previous.get(path)
        const graphic =
          cached?.original === original
            ? cached
            : {
                original,
                source: fadeUsageSvg(
                  original,
                  elapsed,
                  `${this.identity}:${this.generation}:${site.startedAt}`,
                ),
              }
        graphics.set(path, graphic)
        return { ...node, props: { ...node.props, source: graphic.source } }
      }
      if (!("children" in node) || !node.children) return node
      const children = node.children.map((child, index) =>
        visit(child, `${path}.${index}`),
      )
      if (children.every((child, index) => child === node.children![index]))
        return node
      return {
        ...node,
        children,
      } as RenderElement
    }
    const result = visit(drawing, "usage") as T
    if (site) site.graphics = graphics
    return result
  }
}
