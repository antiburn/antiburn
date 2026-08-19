// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { localRepositoryKey } from "../../lib/presentation/localIdentity"
import {
  isRepositorySelected,
  type LocalRepositoryDescriptor,
  type LocalRepositoryItem,
} from "../../lib/types/repository"
import { LocalRepositoryList, type LocalRepositoryListProps } from "./LocalRepositoryList"

afterEach(cleanup)

/**
 * Build a list row the way a host would: from an engine repository descriptor
 * plus what discovery found for it on this machine.
 */
function itemFrom(
  descriptor: LocalRepositoryDescriptor,
  over: Partial<LocalRepositoryItem> = {},
): LocalRepositoryItem {
  return {
    key: localRepositoryKey(descriptor.fullName, over.wslDistro, over.repoRoot),
    repoName: descriptor.name,
    fullName: descriptor.fullName,
    status: "accessible",
    enabled: isRepositorySelected(descriptor),
    ...over,
  }
}

function descriptor(
  name: string,
  over: Partial<LocalRepositoryDescriptor> = {},
): LocalRepositoryDescriptor {
  return {
    name,
    fullName: `avery/${name}`,
    cloneUrl: `https://example.invalid/avery/${name}.git`,
    sshUrl: `git@example.invalid:avery/${name}.git`,
    ...over,
  }
}

function list(over: Partial<LocalRepositoryListProps> = {}) {
  const props: LocalRepositoryListProps = {
    repositories: [itemFrom(descriptor("widgets"), { repoRoot: "/Users/avery/code/widgets" })],
    ...over,
  }
  return render(<LocalRepositoryList {...props} />)
}

describe("LocalRepositoryList — identity", () => {
  it("names each repository and shows where it lives", () => {
    list()
    expect(screen.getByText("avery/widgets")).toBeTruthy()
    expect(screen.getByText("/Users/avery/code/widgets")).toBeTruthy()
  })

  it("distinguishes two clones of one repository by path", () => {
    list({
      repositories: [
        itemFrom(descriptor("widgets"), { repoRoot: "/Users/avery/code/widgets" }),
        itemFrom(descriptor("widgets"), { repoRoot: "/Users/avery/scratch/widgets" }),
      ],
    })
    expect(screen.getAllByText("avery/widgets")).toHaveLength(2)
    expect(screen.getByText("/Users/avery/code/widgets")).toBeTruthy()
    expect(screen.getByText("/Users/avery/scratch/widgets")).toBeTruthy()
  })

  it("marks a WSL clone", () => {
    list({
      repositories: [
        itemFrom(descriptor("widgets"), {
          repoRoot: "/home/avery/widgets",
          wslDistro: "Ubuntu",
        }),
      ],
    })
    expect(screen.getByLabelText("Found in Ubuntu on Windows Subsystem for Linux")).toBeTruthy()
  })

  it("reports worktrees and observed sessions", () => {
    list({
      repositories: [
        itemFrom(descriptor("widgets"), {
          repoRoot: "/Users/avery/code/widgets",
          worktreeCount: 2,
          sessionCount: 1,
        }),
      ],
    })
    expect(screen.getByText("2 worktrees · 1 session")).toBeTruthy()
  })
})

describe("LocalRepositoryList — inclusion", () => {
  it("treats a descriptor with no opinion as included", () => {
    list()
    expect(screen.getByLabelText("Include avery/widgets").getAttribute("aria-checked")).toBe(
      "true",
    )
  })

  it("honours an explicit exclusion", () => {
    list({
      repositories: [
        itemFrom(descriptor("widgets", { enabled: false }), { status: "disabled" }),
      ],
    })
    expect(screen.getByLabelText("Include avery/widgets").getAttribute("aria-checked")).toBe(
      "false",
    )
  })

  it("reports a toggle with the repository and its new state", () => {
    const onToggleRepository = vi.fn()
    list({ onToggleRepository })
    fireEvent.click(screen.getByLabelText("Include avery/widgets"))
    expect(onToggleRepository).toHaveBeenCalledWith(
      expect.objectContaining({ fullName: "avery/widgets" }),
      false,
    )
  })

  it("states inclusion but disables the switch when the host cannot change it", () => {
    list()
    const readOnly = screen.getByLabelText("Include avery/widgets")
    expect(readOnly.getAttribute("aria-checked")).toBe("true")
    expect(readOnly.hasAttribute("disabled")).toBe(true)

    cleanup()
    list({ onToggleRepository: vi.fn() })
    expect(screen.getByLabelText("Include avery/widgets").hasAttribute("disabled")).toBe(false)
  })

  it("offers an undo for the change just made", () => {
    const onUndo = vi.fn()
    list({ undo: { label: "Ignored avery/widgets", onUndo } })
    expect(screen.getByText("Ignored avery/widgets")).toBeTruthy()
    fireEvent.click(screen.getByText("Undo"))
    expect(onUndo).toHaveBeenCalledOnce()
  })

  it("shows no undo bar by default", () => {
    list()
    expect(screen.queryByText("Undo")).toBeNull()
  })
})

describe("LocalRepositoryList — recovery states", () => {
  it("explains a blocked repository and offers to ask for access", () => {
    const onGrantAccess = vi.fn()
    list({
      repositories: [
        itemFrom(descriptor("widgets"), {
          status: "permission_denied",
          suspectedPath: "/Users/avery/Documents/widgets",
        }),
      ],
      onGrantAccess,
    })
    expect(screen.getByLabelText("Access blocked")).toBeTruthy()
    expect(screen.getByText(/Blocked by the system/)).toBeTruthy()
    expect(screen.getByText("/Users/avery/Documents/widgets")).toBeTruthy()

    fireEvent.click(screen.getByText("Grant access"))
    expect(onGrantAccess).toHaveBeenCalledWith(
      expect.objectContaining({ status: "permission_denied" }),
    )
  })

  it("offers to locate a repository that is not on this machine", () => {
    const onLocate = vi.fn()
    list({
      repositories: [itemFrom(descriptor("widgets"), { status: "not_cloned" })],
      onLocate,
    })
    expect(screen.getByText(/Not on this machine/)).toBeTruthy()

    fireEvent.click(screen.getByText("Locate…"))
    expect(onLocate).toHaveBeenCalledWith(expect.objectContaining({ status: "not_cloned" }))
  })

  it("does not offer inclusion for a repository that is not here", () => {
    list({
      repositories: [itemFrom(descriptor("widgets"), { status: "not_cloned" })],
      onToggleRepository: vi.fn(),
    })
    expect(screen.queryByLabelText("Include avery/widgets")).toBeNull()
  })

  it("hides a recovery control the host cannot service", () => {
    list({ repositories: [itemFrom(descriptor("widgets"), { status: "not_cloned" })] })
    expect(screen.queryByText("Locate…")).toBeNull()
  })
})

describe("LocalRepositoryList — loading, empty, refresh", () => {
  it("shows placeholders only on the first scan", () => {
    const { unmount } = list({ repositories: [], loading: true })
    expect(screen.getByTestId("repository-list-placeholder")).toBeTruthy()
    unmount()

    // A rescan over existing rows keeps them on screen rather than blanking.
    list({ loading: true })
    expect(screen.queryByTestId("repository-list-placeholder")).toBeNull()
    expect(screen.getByText("avery/widgets")).toBeTruthy()
  })

  it("explains an empty result and offers to scan again", () => {
    const onRefresh = vi.fn()
    list({ repositories: [], onRefresh })
    expect(screen.getByText("No repositories found")).toBeTruthy()
    fireEvent.click(screen.getByText("Scan again"))
    expect(onRefresh).toHaveBeenCalledOnce()
  })

  it("accepts caller-supplied empty copy", () => {
    list({ repositories: [], emptyTitle: "Nothing yet", emptyDescription: "Run a session." })
    expect(screen.getByText("Nothing yet")).toBeTruthy()
    expect(screen.getByText("Run a session.")).toBeTruthy()
  })

  it("rescans from the header, and disables the control while one is running", () => {
    const onRefresh = vi.fn()
    const { unmount } = list({ onRefresh })
    fireEvent.click(screen.getByLabelText("Rescan for repositories"))
    expect(onRefresh).toHaveBeenCalledOnce()
    unmount()

    list({ onRefresh, loading: true })
    expect(screen.getByLabelText("Rescan for repositories").hasAttribute("disabled")).toBe(true)
  })

  it("hides the rescan control when the host cannot rescan", () => {
    list()
    expect(screen.queryByLabelText("Rescan for repositories")).toBeNull()
  })
})
