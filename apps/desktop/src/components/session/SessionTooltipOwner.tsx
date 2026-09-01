import * as TooltipPrimitive from "@radix-ui/react-tooltip"
import {
  cloneElement,
  isValidElement,
  useCallback,
  useId,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type PointerEvent,
  type ReactElement,
  type ReactNode,
  type Ref,
  type RefCallback,
  type UIEvent,
} from "react"

import {
  SharedTooltipOwnerContext,
  type SharedTooltipOwner,
  type SharedTooltipRegistration,
} from "../presentation/Tooltip"

interface ActiveTooltip extends SharedTooltipRegistration {
  rect: DOMRect
}

type OwnedElementProps = {
  children?: ReactNode
  ref?: Ref<HTMLElement> | undefined
  onPointerOver?: ((event: PointerEvent<HTMLElement>) => void) | undefined
  onPointerOut?: ((event: PointerEvent<HTMLElement>) => void) | undefined
  onPointerDown?: ((event: PointerEvent<HTMLElement>) => void) | undefined
  onClick?: ((event: React.MouseEvent<HTMLElement>) => void) | undefined
  onFocusCapture?: ((event: FocusEvent<HTMLElement>) => void) | undefined
  onBlurCapture?: ((event: FocusEvent<HTMLElement>) => void) | undefined
  onScrollCapture?: ((event: UIEvent<HTMLElement>) => void) | undefined
}

function setRef<T>(ref: Ref<T> | undefined, value: T | null): void | (() => void) {
  if (typeof ref === "function") return ref(value)
  if (ref) ref.current = value
}

function triggerFrom(target: EventTarget | null): HTMLElement | null {
  return target instanceof Element
    ? target.closest<HTMLElement>("[data-shared-tooltip-trigger]")
    : null
}

export function SessionTooltipOwner({ children }: { children: ReactNode }) {
  const registrations = useRef(new WeakMap<HTMLElement, SharedTooltipRegistration>())
  const timer = useRef<number | null>(null)
  const currentNode = useRef<HTMLElement | null>(null)
  const [active, setActive] = useState<ActiveTooltip | null>(null)
  const [open, setOpen] = useState(false)
  const contentId = useId()

  const clearTimer = useCallback(() => {
    if (timer.current === null) return
    window.clearTimeout(timer.current)
    timer.current = null
  }, [])

  const close = useCallback(() => {
    clearTimer()
    const node = currentNode.current
    if (node) {
      node.dataset.state = "closed"
      if (node.getAttribute("aria-describedby") === contentId) {
        node.removeAttribute("aria-describedby")
      }
    }
    currentNode.current = null
    setOpen(false)
    setActive(null)
  }, [clearTimer, contentId])

  const show = useCallback(
    (node: HTMLElement, delayed: boolean) => {
      const registration = registrations.current.get(node)
      if (!registration) return

      close()
      currentNode.current = node
      node.dataset.state = "closed"
      setActive({ ...registration, rect: node.getBoundingClientRect() })

      const openTooltip = () => {
        if (currentNode.current !== node || !node.isConnected) return
        timer.current = null
        node.dataset.state = delayed ? "delayed-open" : "instant-open"
        node.setAttribute("aria-describedby", contentId)
        setOpen(true)
      }

      if (delayed && registration.delayMs > 0) {
        timer.current = window.setTimeout(openTooltip, registration.delayMs)
      } else {
        openTooltip()
      }
    },
    [close, contentId],
  )

  const register = useCallback<SharedTooltipOwner["register"]>(
    (node, registration) => {
      registrations.current.set(node, registration)
      node.dataset.state = "closed"
      if (currentNode.current === node) {
        setActive({ ...registration, rect: node.getBoundingClientRect() })
      }
      return () => {
        registrations.current.delete(node)
        queueMicrotask(() => {
          if (registrations.current.has(node)) return
          if (currentNode.current === node) close()
          delete node.dataset.state
          if (node.getAttribute("aria-describedby") === contentId) {
            node.removeAttribute("aria-describedby")
          }
        })
      }
    },
    [close, contentId],
  )

  const owner = useMemo<SharedTooltipOwner>(() => ({ register }), [register])

  const child = isValidElement(children) ? (children as ReactElement<OwnedElementProps>) : null
  const childRef = child?.props.ref
  const assignBoundaryRef = useCallback<RefCallback<HTMLElement>>(
    (node) => {
      const childCleanup = setRef(childRef, node)
      if (!node) return childCleanup
      const viewport = node.closest(".ui-scroll-viewport")
      viewport?.addEventListener("scroll", close)
      return () => {
        viewport?.removeEventListener("scroll", close)
        close()
        if (childCleanup) childCleanup()
        else setRef(childRef, null)
      }
    },
    [childRef, close],
  )

  if (!child) throw new Error("SessionTooltipOwner requires one element child")

  // React uses this callback only as the cloned element's ref.
  // eslint-disable-next-line react-hooks/refs
  const ownedChild = cloneElement(child, {
    ref: assignBoundaryRef,
    onPointerOver: (event) => {
      child.props.onPointerOver?.(event)
      if (event.defaultPrevented || event.pointerType === "touch") return
      const node = triggerFrom(event.target)
      if (!node || node === triggerFrom(event.relatedTarget)) return
      show(node, true)
    },
    onPointerOut: (event) => {
      child.props.onPointerOut?.(event)
      if (event.defaultPrevented) return
      const node = triggerFrom(event.target)
      if (node && node !== triggerFrom(event.relatedTarget)) close()
    },
    onPointerDown: (event) => {
      child.props.onPointerDown?.(event)
      if (!event.defaultPrevented && triggerFrom(event.target)) close()
    },
    onClick: (event) => {
      child.props.onClick?.(event)
      if (!event.defaultPrevented && triggerFrom(event.target)) close()
    },
    onFocusCapture: (event) => {
      child.props.onFocusCapture?.(event)
      if (event.defaultPrevented) return
      const node = triggerFrom(event.target)
      if (node) show(node, false)
    },
    onBlurCapture: (event) => {
      child.props.onBlurCapture?.(event)
      if (event.defaultPrevented) return
      const node = triggerFrom(event.target)
      if (node && node !== triggerFrom(event.relatedTarget)) close()
    },
    onScrollCapture: (event) => {
      child.props.onScrollCapture?.(event)
      close()
    },
  })

  return (
    <SharedTooltipOwnerContext value={owner}>
      {ownedChild}
      <TooltipPrimitive.Provider>
        <TooltipPrimitive.Root open={open} onOpenChange={(nextOpen) => !nextOpen && close()}>
          <TooltipPrimitive.Trigger asChild>
            <span
              data-session-tooltip-owner=""
              aria-hidden="true"
              tabIndex={-1}
              style={{
                position: "fixed",
                pointerEvents: "none",
                opacity: 0,
                top: active?.rect.top ?? 0,
                left: active?.rect.left ?? 0,
                width: active?.rect.width ?? 0,
                height: active?.rect.height ?? 0,
              }}
            />
          </TooltipPrimitive.Trigger>
          <TooltipPrimitive.Portal>
            <TooltipPrimitive.Content
              id={contentId}
              side={active?.side ?? "top"}
              sideOffset={4}
              collisionPadding={8}
              className="ui-tooltip max-w-[220px] whitespace-normal"
            >
              {active?.label}
            </TooltipPrimitive.Content>
          </TooltipPrimitive.Portal>
        </TooltipPrimitive.Root>
      </TooltipPrimitive.Provider>
    </SharedTooltipOwnerContext>
  )
}
