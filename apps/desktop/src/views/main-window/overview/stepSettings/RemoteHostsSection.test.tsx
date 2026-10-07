import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { RemoteHost, RemoteHostsSnapshot } from "../../../../lib/remoteHosts"
import { RemoteHostsSection } from "./RemoteHostsSection"
import type { SourcesSession } from "./SourcesSession"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))

function host(over: Partial<RemoteHost> = {}): RemoteHost {
  return {
    id: "host-a",
    sshAlias: "alpha",
    displayName: "Alpha",
    status: "idle",
    lastSuccessfulSyncEpoch: 1_799_000_000,
    automaticSyncEnabled: true,
    cachedSessionCount: 4,
    lastError: null,
    ...over,
  }
}

function snapshot(hosts: RemoteHost[] = []): RemoteHostsSnapshot {
  return {
    hosts,
    loaded: true,
    loading: false,
    sync: { intervalSecs: 300, active: null, pendingHostIds: [] },
  }
}

function fakeSession(over: Partial<SourcesSession> = {}): SourcesSession {
  return {
    checkRemoteHost: vi.fn(),
    addRemoteHost: vi.fn(),
    updateRemoteHost: vi.fn(),
    removeRemoteHost: vi.fn(),
    scanRemoteHost: vi.fn(),
    setRemoteHostSyncEnabled: vi.fn(),
    setRemoteSyncInterval: vi.fn(),
    ...over,
  } as unknown as SourcesSession
}

beforeEach(() => {
  vi.clearAllMocks()
  invoke.mockResolvedValue(undefined)
})

describe("RemoteHostsSection", () => {
  it("groups host creation and the searchable sync control with remote hosts", () => {
    const setRemoteSyncInterval = vi.fn()
    render(
      <RemoteHostsSection
        remote={snapshot()}
        session={fakeSession({ setRemoteSyncInterval })}
        appVersion="0.8.0"
      />,
    )
    const group = screen.getByText("Remote hosts").closest("[data-settings-control]")
    expect(group).not.toBeNull()
    const controls = within(group as HTMLElement)
    expect(controls.getByRole("button", { name: "Add host" })).toBeVisible()
    expect(controls.queryByRole("heading", { name: "Automatic sync" })).toBeNull()
    const interval = controls.getByRole("combobox", { name: "Sync frequency" })
    expect(interval.closest("[data-settings-control]")).toHaveAttribute(
      "data-settings-control",
      "sourceAutomaticSync",
    )
    fireEvent.change(interval, { target: { value: "900" } })
    expect(setRemoteSyncInterval).toHaveBeenCalledWith(900)
  })

  it("does not repeat the SSH alias when it is already the host name", () => {
    render(
      <RemoteHostsSection
        remote={snapshot([host({ displayName: null })])}
        session={fakeSession()}
        appVersion="0.8.0"
      />,
    )
    expect(screen.getAllByText("alpha", { exact: true })).toHaveLength(1)
    expect(screen.getByRole("button", { name: "Sync now" })).toBeVisible()
  })

  it("keeps last-good metadata visible after a failure and offers a stable retry", () => {
    const scanRemoteHost = vi.fn(async () => undefined)
    render(
      <RemoteHostsSection
        remote={snapshot([
          host({
            status: "error",
            lastError: { category: "transferFailed", message: "Transfer interrupted." },
          }),
        ])}
        session={fakeSession({ scanRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    expect(screen.getByText("4 sessions", { exact: false })).toBeVisible()
    expect(screen.getByText(/^Synced (just now|\d)/)).toBeVisible()
    expect(screen.getByText(/Last sync failed · Transfer interrupted/)).toBeVisible()
    const retry = screen.getByRole("button", { name: "Retry" })
    expect(retry).toBeEnabled()
    fireEvent.click(retry)
    expect(scanRemoteHost).toHaveBeenCalledWith("host-a")
  })

  it("keeps the saved interval after a failed change and supports retry", async () => {
    const setRemoteSyncInterval = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(undefined)
    render(
      <RemoteHostsSection
        remote={snapshot([host()])}
        session={fakeSession({ setRemoteSyncInterval })}
        appVersion="0.8.0"
      />,
    )
    const interval = screen.getByRole("combobox", { name: "Sync frequency" })
    fireEvent.change(interval, { target: { value: "900" } })
    expect(await screen.findByRole("alert")).toHaveTextContent("Still set to every 5 minutes")
    expect(interval).toHaveValue("300")
    expect(interval).toBeEnabled()
    fireEvent.change(interval, { target: { value: "900" } })
    await waitFor(() => expect(interval).toBeEnabled())
    expect(screen.queryByRole("alert")).toBeNull()
    expect(setRemoteSyncInterval).toHaveBeenCalledTimes(2)
  })

  it("retains cached sessions when starting sync fails and opens the host filter", async () => {
    const scanRemoteHost = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(undefined)
    render(
      <RemoteHostsSection
        remote={snapshot([host()])}
        session={fakeSession({ scanRemoteHost })}
        appVersion="0.8.0"
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Sync now" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Your saved sessions are still available",
    )
    fireEvent.click(
      screen.getByRole("button", {
        name: "Browse sessions from Alpha in the current date range",
      }),
    )
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_main_window_section", {
        section: "activity",
        remoteHostId: "host-a",
      }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
  })

  it("explains manual sync and disables navigation for an empty host", () => {
    const remote = snapshot([host({ cachedSessionCount: 0 })])
    remote.sync.intervalSecs = 0
    render(<RemoteHostsSection remote={remote} session={fakeSession()} appVersion="0.8.0" />)
    expect(screen.getByText("Use Sync now on a host to update its sessions.")).toBeVisible()
    expect(
      screen.getByRole("button", {
        name: "Browse sessions from Alpha in the current date range",
      }),
    ).toBeDisabled()
  })

  it("restores Sync now when the scheduler finishes before the host update arrives", () => {
    const remote = snapshot([host({ status: "syncing" })])
    remote.sync.active = { hostId: "host-a", completed: 0, total: 2 }
    const session = fakeSession()
    const { rerender } = render(
      <RemoteHostsSection remote={remote} session={session} appVersion="0.8.0" />,
    )
    expect(screen.getByRole("button", { name: "Syncing…" })).toBeDisabled()
    rerender(
      <RemoteHostsSection
        remote={{ ...remote, sync: { ...remote.sync, active: null } }}
        session={session}
        appVersion="0.8.0"
      />,
    )
    expect(screen.getByRole("button", { name: "Sync now" })).toBeEnabled()
  })

  it("disables all host syncing while preserving cached sessions", async () => {
    const setRemoteHostSyncEnabled = vi.fn().mockResolvedValue(undefined)
    const scanRemoteHost = vi.fn()
    const session = fakeSession({ setRemoteHostSyncEnabled, scanRemoteHost })
    const { rerender } = render(
      <RemoteHostsSection remote={snapshot([host()])} session={session} appVersion="0.8.0" />,
    )
    const toggle = screen.getByRole("switch", { name: "Sync for Alpha" })
    expect(toggle).toBeChecked()
    fireEvent.click(toggle)
    await waitFor(() => expect(setRemoteHostSyncEnabled).toHaveBeenCalledWith("host-a", false))
    rerender(
      <RemoteHostsSection
        remote={snapshot([host({ automaticSyncEnabled: false })])}
        session={session}
        appVersion="0.8.0"
      />,
    )
    expect(toggle).not.toBeChecked()
    expect(screen.getByText("Sync off")).toBeVisible()
    expect(screen.getByText("Sync off")).toHaveAttribute("tabindex", "0")
    expect(screen.getByText("Sync off").querySelector(".sr-only")).toHaveTextContent(/Synced /)
    expect(screen.getByText("4 sessions", { exact: true })).toBeVisible()
    expect(screen.getByRole("button", { name: "Sync now" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Sync now" }))
    expect(scanRemoteHost).not.toHaveBeenCalled()
    await waitFor(() => expect(toggle).toBeEnabled())
    fireEvent.click(toggle)
    await waitFor(() => expect(setRemoteHostSyncEnabled).toHaveBeenCalledWith("host-a", true))
    rerender(
      <RemoteHostsSection remote={snapshot([host()])} session={session} appVersion="0.8.0" />,
    )
    expect(screen.getByRole("button", { name: "Sync now" })).toBeEnabled()
    fireEvent.click(screen.getByRole("button", { name: "Sync now" }))
    await waitFor(() => expect(scanRemoteHost).toHaveBeenCalledWith("host-a"))
  })

  it("retains the saved toggle value after failure and allows retry", async () => {
    const setRemoteHostSyncEnabled = vi
      .fn()
      .mockRejectedValueOnce(new Error("unavailable"))
      .mockResolvedValueOnce(undefined)
    render(
      <RemoteHostsSection
        remote={snapshot([host()])}
        session={fakeSession({ setRemoteHostSyncEnabled })}
        appVersion="0.8.0"
      />,
    )
    const toggle = screen.getByRole("switch", { name: "Sync for Alpha" })
    fireEvent.click(toggle)
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Your previous setting is unchanged",
    )
    expect(toggle).toBeChecked()
    expect(toggle).toBeEnabled()
    fireEvent.click(toggle)
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
    expect(setRemoteHostSyncEnabled).toHaveBeenCalledTimes(2)
  })

  it("guides missing-helper setup with the versioned architecture asset, then saves", async () => {
    const checkRemoteHost = vi
      .fn()
      .mockResolvedValueOnce({
        status: "helperMissing",
        supportedAgents: [],
        platform: "linux",
        architecture: "aarch64",
        helperVersion: null,
        message: "Install the remote helper.",
      })
      .mockResolvedValueOnce({
        status: "ready",
        supportedAgents: ["claude-code", "codex"],
        platform: "linux",
        architecture: "aarch64",
        helperVersion: null,
        message: null,
      })
    const addRemoteHost = vi.fn(async () => host())
    render(
      <RemoteHostsSection
        remote={snapshot()}
        session={fakeSession({ checkRemoteHost, addRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Add host" }))
    const dialog = screen.getByRole("dialog", { name: "Add remote host" })
    fireEvent.change(within(dialog).getByLabelText("SSH host alias"), {
      target: { value: "alpha" },
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Check connection" }))
    expect(
      await within(dialog).findAllByText(
        "antiburn-remote-0.8.0-aarch64-unknown-linux-musl.tar.gz",
        { exact: false },
      ),
    ).not.toHaveLength(0)
    expect(
      within(dialog).getByText(/sha256sum --check --ignore-missing SHA256SUMS/),
    ).toBeVisible()
    expect(within(dialog).getByText(/no.*persistent service/i)).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: /Open official Releases/ }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_remote_helper_downloads"))
    fireEvent.click(within(dialog).getByRole("button", { name: "Check again" }))
    expect(
      await within(dialog).findByText(/Linux ARM64 · Found Claude Code and Codex/),
    ).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: "Add host" }))
    await waitFor(() => expect(addRemoteHost).toHaveBeenCalledWith("alpha", null))
  })

  it("saves a name-only edit without an SSH preflight", async () => {
    const checkRemoteHost = vi.fn()
    const updateRemoteHost = vi.fn(async () => host({ displayName: "Renamed" }))
    render(
      <RemoteHostsSection
        remote={snapshot([host()])}
        session={fakeSession({ checkRemoteHost, updateRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    fireEvent.pointerDown(screen.getByRole("button", { name: "More actions for Alpha" }), {
      button: 0,
      ctrlKey: false,
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }))
    const dialog = screen.getByRole("dialog", { name: "Edit remote host" })
    fireEvent.change(within(dialog).getByLabelText(/Name/), { target: { value: "Renamed" } })
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }))
    await waitFor(() =>
      expect(updateRemoteHost).toHaveBeenCalledWith("host-a", "alpha", "Renamed"),
    )
    expect(checkRemoteHost).not.toHaveBeenCalled()
  })

  it("requires an architecture choice when the host cannot report one", async () => {
    const checkRemoteHost = vi.fn(async () => ({
      status: "helperMissing" as const,
      supportedAgents: [],
      platform: "linux" as const,
      architecture: null,
      helperVersion: null,
      message: "Install the remote helper.",
    }))
    render(
      <RemoteHostsSection
        remote={snapshot()}
        session={fakeSession({ checkRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Add host" }))
    const dialog = screen.getByRole("dialog", { name: "Add remote host" })
    fireEvent.change(within(dialog).getByLabelText("SSH host alias"), {
      target: { value: "alpha" },
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Check connection" }))
    expect(await within(dialog).findByText("Choose the Linux host architecture")).toBeVisible()
    expect(within(dialog).queryByText(/x86_64-unknown-linux-musl\.tar\.gz/)).toBeNull()

    fireEvent.click(within(dialog).getByRole("radio", { name: "ARM64" }))
    expect(
      within(dialog).getAllByText("antiburn-remote-0.8.0-aarch64-unknown-linux-musl.tar.gz", {
        exact: false,
      }),
    ).not.toHaveLength(0)
    expect(
      within(dialog).getByText(/sha256sum --check --ignore-missing SHA256SUMS/),
    ).toBeVisible()
  })

  it("locks identity fields while an asynchronous preflight is pending", async () => {
    let finishCheck:
      | ((value: {
          status: "ready"
          supportedAgents: ["codex"]
          platform: "linux"
          architecture: "x86_64"
          helperVersion: null
          message: null
        }) => void)
      | undefined
    const checkRemoteHost = vi.fn(
      () =>
        new Promise<{
          status: "ready"
          supportedAgents: ["codex"]
          platform: "linux"
          architecture: "x86_64"
          helperVersion: null
          message: null
        }>((resolve) => {
          finishCheck = resolve
        }),
    )
    render(
      <RemoteHostsSection
        remote={snapshot()}
        session={fakeSession({ checkRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Add host" }))
    const dialog = screen.getByRole("dialog", { name: "Add remote host" })
    const alias = within(dialog).getByLabelText("SSH host alias")
    fireEvent.change(alias, { target: { value: "alpha" } })
    fireEvent.click(within(dialog).getByRole("button", { name: "Check connection" }))

    await waitFor(() => expect(alias).toBeDisabled())
    expect(within(dialog).getByLabelText(/Name/)).toBeDisabled()
    expect(within(dialog).getAllByRole("button", { name: "Cancel" })).toHaveLength(1)
    expect(within(dialog).getByRole("button", { name: "Checking…" })).toBeDisabled()
    finishCheck?.({
      status: "ready",
      supportedAgents: ["codex"],
      platform: "linux",
      architecture: "x86_64",
      helperVersion: null,
      message: null,
    })
    expect(await within(dialog).findByText(/Linux x64 · Found Codex/)).toBeVisible()
  })

  it("names the helper version the host reports", async () => {
    const checkRemoteHost = vi.fn(async () => ({
      status: "ready" as const,
      supportedAgents: ["codex" as const],
      platform: "linux" as const,
      architecture: "x86_64" as const,
      helperVersion: "0.9.0",
      message: null,
    }))
    render(
      <RemoteHostsSection
        remote={snapshot()}
        session={fakeSession({ checkRemoteHost })}
        appVersion="0.8.0"
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Add host" }))
    const dialog = screen.getByRole("dialog", { name: "Add remote host" })
    fireEvent.change(within(dialog).getByLabelText("SSH host alias"), {
      target: { value: "alpha" },
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Check connection" }))

    expect(
      await within(dialog).findByText(/Linux x64 · Helper 0\.9\.0 · Found Codex/),
    ).toBeVisible()
  })

  it.each(["add", "remove"])(
    "restores the app when an open %s dialog unmounts",
    async (mode) => {
      const appRoot = document.createElement("div")
      appRoot.id = "root"
      document.body.append(appRoot)
      const result = render(
        <RemoteHostsSection
          remote={snapshot([host()])}
          session={fakeSession()}
          appVersion="0.8.0"
        />,
        { container: appRoot },
      )
      try {
        if (mode === "add") {
          fireEvent.click(screen.getByRole("button", { name: "Add host" }))
        } else {
          fireEvent.pointerDown(
            screen.getByRole("button", { name: "More actions for Alpha" }),
            {
              button: 0,
              ctrlKey: false,
            },
          )
          fireEvent.click(await screen.findByRole("menuitem", { name: "Remove" }))
        }
        expect(appRoot).toHaveAttribute("inert")
        result.unmount()
        expect(appRoot).not.toHaveAttribute("inert")
        expect(screen.queryByRole(mode === "add" ? "dialog" : "alertdialog")).toBeNull()
      } finally {
        result.unmount()
        appRoot.remove()
      }
    },
  )

  it("makes the app inert while editing and restores focus to the opener", async () => {
    const appRoot = document.createElement("div")
    appRoot.id = "root"
    document.body.append(appRoot)
    const result = render(
      <RemoteHostsSection remote={snapshot()} session={fakeSession()} appVersion="0.8.0" />,
      { container: appRoot },
    )
    const opener = screen.getByRole("button", { name: "Add host" })
    opener.focus()

    fireEvent.click(opener)
    expect(appRoot).toHaveAttribute("inert")
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }))
    await waitFor(() => expect(document.activeElement).toBe(opener))
    expect(appRoot).not.toHaveAttribute("inert")

    result.unmount()
    appRoot.remove()
  })
})
