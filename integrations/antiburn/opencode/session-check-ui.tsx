/** @jsxImportSource @opentui/solid */
import type { TuiPluginModule } from '@opencode-ai/plugin/tui'
import { ErrorBoundary, Show, createEffect, createSignal, onCleanup, onMount } from 'solid-js'
import { useTerminalDimensions } from '@opentui/solid'
import { SyntaxStyle, type MarkdownRenderable } from '@opentui/core'
import {
  createOpenCodeUiController,
  resolveOpenCodeUiSession,
  openCodeMarkdownStyles,
  type AdvisoryFrame,
} from './session-check-ui.mjs'

const plugin: TuiPluginModule = {
  id: 'antiburn.session-check-ui',
  async tui(api) {
    if (api.app.version !== '1.19.3') return
    const [frame, setFrame] = createSignal<AdvisoryFrame>()
    const sessionId = () => {
      const route = api.route.current
      return route.name === 'session' && typeof route.params?.sessionID === 'string'
        ? route.params.sessionID
        : undefined
    }
    const controller = createOpenCodeUiController({
      currentSessionId: sessionId,
      resolveSession: (id) => resolveOpenCodeUiSession(api, id),
      onFrame: setFrame,
    })
    const cleanups: Array<() => void> = []
    api.lifecycle.onDispose(() => {
      for (const cleanup of cleanups) cleanup()
      controller.dispose()
    })
    cleanups.push(
      api.event.on('message.updated', ({ properties }) => {
        if (properties.info.sessionID === sessionId()) void controller.refreshFromMessage()
      }),
      api.event.on('session.idle', ({ properties }) => {
        if (properties.sessionID === sessionId()) void controller.refresh()
      }),
      api.event.on('session.deleted', ({ properties }) => {
        if (properties.info.id === sessionId()) controller.activate(undefined)
      }),
    )
    const AdvisorySlot = () => {
      const dimensions = useTerminalDimensions()
      let timer: ReturnType<typeof setTimeout> | undefined
      let alive = true
      const poll = () => {
        if (!alive) return
        if (sessionId()) void controller.refresh()
        timer = setTimeout(poll, 5000)
      }
      createEffect(() => controller.activate(sessionId()))
      onMount(poll)
      onCleanup(() => {
        alive = false
        if (timer !== undefined) clearTimeout(timer)
        controller.activate(undefined)
      })
      const handoff = (value: AdvisoryFrame) => {
        onMount(() => {
          const cleanup = controller.mounted(value)
          onCleanup(cleanup)
        })
      }
      const PlainText = (props: { value: AdvisoryFrame }) => {
        handoff(props.value)
        return <text wrapMode="word">{props.value.text}</text>
      }
      const Markdown = (props: { value: AdvisoryFrame }) => {
        const syntaxStyle = SyntaxStyle.fromStyles(openCodeMarkdownStyles(api.theme.current))
        onCleanup(() => syntaxStyle.destroy())
        let element: MarkdownRenderable | undefined
        createEffect(() => {
          for (const [name, style] of Object.entries(openCodeMarkdownStyles(api.theme.current)))
            syntaxStyle.registerStyle(name, style)
          syntaxStyle.clearCache()
          element?.refreshStyles()
        })
        handoff(props.value)
        return (
          <markdown
            ref={(value) => {
              element = value
            }}
            content={props.value.markdown}
            syntaxStyle={syntaxStyle}
            fg={api.theme.current.markdownText}
            streaming={false}
            conceal={true}
          />
        )
      }
      const Advisory = (props: { value: AdvisoryFrame }) => (
        <scrollbox
          height={Math.max(1, Math.min(12, Math.floor(dimensions().height / 3)))}
          flexShrink={0}
          scrollX={false}
          paddingLeft={1}
          paddingRight={1}
        >
          <ErrorBoundary fallback={() => <PlainText value={props.value} />}>
            <Markdown value={props.value} />
          </ErrorBoundary>
        </scrollbox>
      )
      return (
        <box flexDirection="column" flexShrink={0}>
          <Show when={frame()?.sessionId === sessionId() ? frame() : undefined} keyed>
            {(value) => <Advisory value={value} />}
          </Show>
        </box>
      )
    }
    api.slots.register({
      slots: { app_bottom: () => <AdvisorySlot /> },
    })
  },
}

export default plugin
