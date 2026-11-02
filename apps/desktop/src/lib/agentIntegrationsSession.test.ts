import { beforeEach, expect, it, vi } from "vitest"
import { AgentIntegrationsSession } from "./agentIntegrationsSession"
import type { IntegrationStatus } from "./agentIntegrationsIpc"

const api = vi.hoisted(() => ({
  getAgentIntegrations: vi.fn(),
  onAgentIntegrationsChanged: vi.fn(),
  repairAgentIntegration: vi.fn(),
  removeAgentIntegration: vi.fn(),
}))
vi.mock("./agentIntegrationsIpc", () => api)

const pending: IntegrationStatus[] = [
  {
    vendor: "codex",
    phase: "pending_native_gate",
    reason: "native_gate_pending",
    hostVersion: "test",
    profile: null,
    nodeVersion: null,
    owned: false,
  },
]

beforeEach(() => {
  vi.resetAllMocks()
  api.getAgentIntegrations.mockResolvedValue(pending)
  api.onAgentIntegrationsChanged.mockResolvedValue(() => {})
})

async function flushRequests(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

it("binds events before reading and keeps a snapshot received during a request", async () => {
  const order: string[] = []
  let changed = (_statuses: IntegrationStatus[]) => {}
  let resolveFirst: (value: IntegrationStatus[]) => void = () => {}
  api.onAgentIntegrationsChanged.mockImplementation(async (callback: typeof changed) => {
    order.push("listen")
    changed = callback
    return () => {}
  })
  api.getAgentIntegrations.mockImplementationOnce(() => {
    order.push("get")
    return new Promise<IntegrationStatus[]>((resolve) => {
      resolveFirst = resolve
    })
  })
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(order).toEqual(["listen", "get"]))
  changed(pending)
  resolveFirst([])
  await vi.waitFor(() => expect(session.getSnapshot().statuses).toEqual(pending))
  expect(api.getAgentIntegrations).toHaveBeenCalledTimes(1)
  expect(api.repairAgentIntegration).not.toHaveBeenCalled()
  expect(api.removeAgentIntegration).not.toHaveBeenCalled()
  stop()
})

it("loads and listens once for all subscribers, then releases the listener", async () => {
  const unlisten = vi.fn()
  api.onAgentIntegrationsChanged.mockResolvedValue(unlisten)
  const session = new AgentIntegrationsSession()
  expect(api.getAgentIntegrations).not.toHaveBeenCalled()
  expect(api.onAgentIntegrationsChanged).not.toHaveBeenCalled()
  const stop = session.subscribe(() => {})
  const stopAgain = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  expect(api.getAgentIntegrations).toHaveBeenCalledOnce()
  expect(api.onAgentIntegrationsChanged).toHaveBeenCalledOnce()
  stop()
  expect(unlisten).not.toHaveBeenCalled()
  stopAgain()
  expect(unlisten).toHaveBeenCalledOnce()
})

it("uses the repair snapshot and its event without another status inspection", async () => {
  let changed = (_statuses: IntegrationStatus[]) => {}
  api.onAgentIntegrationsChanged.mockImplementation(async (callback: typeof changed) => {
    changed = callback
    return () => {}
  })
  const repaired: IntegrationStatus[] = [{ ...pending[0]!, phase: "registered", owned: true }]
  api.repairAgentIntegration.mockImplementation(async () => {
    changed(repaired)
    return repaired
  })
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  await session.repair("codex")
  expect(session.getSnapshot().statuses).toEqual(repaired)
  expect(api.getAgentIntegrations).toHaveBeenCalledOnce()
  expect(session.getSnapshot().error).toBeNull()
  stop()
})

it("keeps external updates received during and after repair", async () => {
  let changed = (_statuses: IntegrationStatus[]) => {}
  let finish!: (statuses: IntegrationStatus[]) => void
  api.onAgentIntegrationsChanged.mockImplementation(async (callback: typeof changed) => {
    changed = callback
    return () => {}
  })
  api.repairAgentIntegration.mockImplementation(
    () => new Promise<IntegrationStatus[]>((resolve) => (finish = resolve)),
  )
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  const repair = session.repair("codex")
  const repaired: IntegrationStatus[] = [{ ...pending[0]!, phase: "registered", owned: true }]
  const external: IntegrationStatus[] = [{ ...pending[0]!, phase: "reload_required" }]
  changed(repaired)
  changed(external)
  finish(repaired)
  await repair
  expect(session.getSnapshot().statuses).toEqual(external)
  changed(pending)
  expect(session.getSnapshot().statuses).toEqual(pending)
  expect(api.getAgentIntegrations).toHaveBeenCalledOnce()
  stop()
})

it("reconciles a failed repair after its event and keeps the command error", async () => {
  let changed = (_statuses: IntegrationStatus[]) => {}
  api.onAgentIntegrationsChanged.mockImplementation(async (callback: typeof changed) => {
    changed = callback
    return () => {}
  })
  const reconciled: IntegrationStatus[] = [{ ...pending[0]!, phase: "user_changed" }]
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  api.getAgentIntegrations.mockResolvedValueOnce(reconciled)
  api.repairAgentIntegration.mockImplementation(async () => {
    changed(pending)
    throw new Error("private profile path")
  })
  await session.repair("codex")
  expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2)
  expect(session.getSnapshot().statuses).toEqual(reconciled)
  expect(session.getSnapshot().busy).toBe(false)
  expect(session.getSnapshot().error).toBe("Could not update agent integrations. Try again.")
  stop()
})

it("keeps status and the command error if repair and reconciliation both fail", async () => {
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  api.repairAgentIntegration.mockRejectedValueOnce(new Error("private repair error"))
  api.getAgentIntegrations.mockRejectedValueOnce(new Error("private status error"))
  await session.repair("codex")
  expect(session.getSnapshot().statuses).toEqual(pending)
  expect(session.getSnapshot().busy).toBe(false)
  expect(session.getSnapshot().error).toBe("Could not update agent integrations. Try again.")
  stop()
})

it("does not let old event callbacks update a remounted session", async () => {
  const callbacks: ((statuses: IntegrationStatus[]) => void)[] = []
  api.onAgentIntegrationsChanged.mockImplementation(async (callback) => {
    callbacks.push(callback)
    return () => {}
  })
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  stop()
  const stopAgain = session.subscribe(() => {})
  await vi.waitFor(() => expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2))
  callbacks[0]!([{ ...pending[0]!, reason: "stale" }])
  expect(session.getSnapshot().statuses).toEqual(pending)
  stopAgain()
})

it("cleans a listener that finishes registration after unmount", async () => {
  const unlisten = vi.fn()
  let resolveListen: (stop: () => void) => void = () => {}
  api.onAgentIntegrationsChanged.mockImplementation(
    () =>
      new Promise<() => void>((resolve) => {
        resolveListen = resolve
      }),
  )
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  stop()
  resolveListen(unlisten)
  await vi.waitFor(() => expect(unlisten).toHaveBeenCalledOnce())
  expect(api.getAgentIntegrations).not.toHaveBeenCalled()
})

it("retains status and hides raw errors when an explicit repair fails", async () => {
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().statuses).toEqual(pending))
  api.repairAgentIntegration.mockRejectedValue(new Error("private profile path"))
  await session.repair("codex")
  expect(session.getSnapshot().statuses).toEqual(pending)
  expect(session.getSnapshot().error).toBe("Could not update agent integrations. Try again.")
  stop()
})

it.each(["success", "failure"])(
  "loads fresh state after remount while an old %s is pending",
  async (result) => {
    let resolveOld: (value: IntegrationStatus[]) => void = () => {}
    let rejectOld: (error: Error) => void = () => {}
    let resolveFresh: (value: IntegrationStatus[]) => void = () => {}
    api.getAgentIntegrations
      .mockImplementationOnce(
        () =>
          new Promise<IntegrationStatus[]>((resolve, reject) => {
            resolveOld = resolve
            rejectOld = reject
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<IntegrationStatus[]>((resolve) => {
            resolveFresh = resolve
          }),
      )
    const session = new AgentIntegrationsSession()
    const stop = session.subscribe(() => {})
    await vi.waitFor(() => expect(api.getAgentIntegrations).toHaveBeenCalledTimes(1))
    stop()
    const snapshots: ReturnType<typeof session.getSnapshot>[] = []
    const stopAgain = session.subscribe(() => snapshots.push(session.getSnapshot()))
    await vi.waitFor(() => expect(api.onAgentIntegrationsChanged).toHaveBeenCalledTimes(2))
    if (result === "success") resolveOld([{ ...pending[0]!, reason: "stale" }])
    else rejectOld(new Error("old request failed"))
    await vi.waitFor(() => expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2))
    expect(
      snapshots.some(
        (snapshot) =>
          snapshot.statuses.some((status) => status.reason === "stale") ||
          snapshot.error !== null,
      ),
    ).toBe(false)
    resolveFresh(pending)
    await vi.waitFor(() => expect(session.getSnapshot().statuses).toEqual(pending))
    expect(session.getSnapshot().busy).toBe(false)
    stopAgain()
  },
)

it.each(["success", "failure"])(
  "does not publish an old repair %s or clear a new load's busy state after remount",
  async (result) => {
    let resolveRepair!: (value: IntegrationStatus[]) => void
    let rejectRepair!: (error: Error) => void
    let resolveFresh!: (value: IntegrationStatus[]) => void
    const session = new AgentIntegrationsSession()
    const stop = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
    api.repairAgentIntegration.mockImplementationOnce(
      () =>
        new Promise<IntegrationStatus[]>((resolve, reject) => {
          resolveRepair = resolve
          rejectRepair = reject
        }),
    )
    const repair = session.repair("codex")
    stop()
    api.getAgentIntegrations.mockImplementationOnce(
      () => new Promise<IntegrationStatus[]>((resolve) => (resolveFresh = resolve)),
    )
    const stopAgain = session.subscribe(() => {})
    await vi.waitFor(() => expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2))
    if (result === "success") resolveRepair([{ ...pending[0]!, reason: "stale repair" }])
    else rejectRepair(new Error("private old repair error"))
    await repair
    expect(session.getSnapshot()).toEqual({ statuses: pending, busy: true, error: null })
    const fresh: IntegrationStatus[] = [{ ...pending[0]!, phase: "reload_required" }]
    resolveFresh(fresh)
    await vi.waitFor(() =>
      expect(session.getSnapshot()).toEqual({ statuses: fresh, busy: false, error: null }),
    )
    expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2)
    stopAgain()
  },
)

it("does not reconcile an unmounted repair failure during the next subscription handshake", async () => {
  let rejectRepair!: (error: Error) => void
  let resolveStale!: (value: IntegrationStatus[]) => void
  let resolveListen!: (stop: () => void) => void
  const staleRead = new Promise<IntegrationStatus[]>((resolve) => (resolveStale = resolve))
  const stale: IntegrationStatus[] = [{ ...pending[0]!, reason: "stale inspection" }]
  const fresh: IntegrationStatus[] = [{ ...pending[0]!, phase: "registered", owned: true }]
  const session = new AgentIntegrationsSession()
  const stop = session.subscribe(() => {})
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  api.repairAgentIntegration.mockImplementationOnce(
    () => new Promise<IntegrationStatus[]>((_resolve, reject) => (rejectRepair = reject)),
  )
  let remounting = false
  api.getAgentIntegrations.mockImplementation(() =>
    remounting ? Promise.resolve(fresh) : staleRead,
  )
  const repair = session.repair("codex")
  stop()
  rejectRepair(new Error("private repair error"))
  await flushRequests()
  const readsWhileStopped = api.getAgentIntegrations.mock.calls.length - 1

  api.onAgentIntegrationsChanged.mockImplementationOnce(
    () => new Promise<() => void>((resolve) => (resolveListen = resolve)),
  )
  remounting = true
  const stopAgain = session.subscribe(() => {})
  resolveStale(stale)
  await flushRequests()
  resolveListen(() => {})
  await repair
  await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  expect(session.getSnapshot()).toEqual({ statuses: fresh, busy: false, error: null })
  expect(readsWhileStopped).toBe(0)
  expect(api.getAgentIntegrations).toHaveBeenCalledTimes(2)
  stopAgain()
})

it.each(["success", "failure"])(
  "keeps a repair %s current when one of two subscribers leaves",
  async (result) => {
    let resolveRepair!: (value: IntegrationStatus[]) => void
    let rejectRepair!: (error: Error) => void
    const unlisten = vi.fn()
    api.onAgentIntegrationsChanged.mockResolvedValue(unlisten)
    const session = new AgentIntegrationsSession()
    const stop = session.subscribe(() => {})
    const stopAgain = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().busy).toBe(false))
    api.repairAgentIntegration.mockImplementationOnce(
      () =>
        new Promise<IntegrationStatus[]>((resolve, reject) => {
          resolveRepair = resolve
          rejectRepair = reject
        }),
    )
    const repair = session.repair("codex")
    stop()
    stop()
    expect(unlisten).not.toHaveBeenCalled()
    expect(session.getSnapshot().busy).toBe(true)
    const current: IntegrationStatus[] = [{ ...pending[0]!, phase: "reload_required" }]
    if (result === "success") resolveRepair(current)
    else {
      api.getAgentIntegrations.mockResolvedValueOnce(current)
      rejectRepair(new Error("private repair error"))
    }
    await repair
    expect(session.getSnapshot()).toEqual({
      statuses: current,
      busy: false,
      error: result === "failure" ? "Could not update agent integrations. Try again." : null,
    })
    expect(api.getAgentIntegrations).toHaveBeenCalledTimes(result === "success" ? 1 : 2)
    expect(api.onAgentIntegrationsChanged).toHaveBeenCalledOnce()
    stopAgain()
    expect(unlisten).toHaveBeenCalledOnce()
  },
)
