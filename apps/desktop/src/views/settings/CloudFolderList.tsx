import { Component, createRef, type ReactNode } from "react"
import type { CloudProjectGroup } from "../../lib/cloud"

type FolderFocus = {
  control: Element
  workspace: string
} & ({ exactPath: string } | { groupPaths: readonly string[] })

/** Keep focus on the same saved permission when discovery changes its row. */
export class CloudFolderList extends Component<{
  children: ReactNode
  groups: readonly CloudProjectGroup[] | undefined
}> {
  private list = createRef<HTMLDivElement>()

  getSnapshotBeforeUpdate(previous: Readonly<typeof this.props>): FolderFocus | null {
    const control = document.activeElement
    if (
      !document.hasFocus() ||
      !control?.matches('[role="switch"], select') ||
      !this.list.current?.contains(control)
    )
      return null
    const workspace =
      control.closest<HTMLElement>("[data-cloud-workspace]")?.dataset.cloudWorkspace
    if (!workspace) return null
    const exactPath = control.closest<HTMLElement>("[data-exact-folder]")?.dataset.exactFolder
    if (exactPath) return { control, exactPath, workspace }
    const groupId = control.closest<HTMLElement>("[data-cloud-folder]")?.dataset.cloudFolder
    const group = previous.groups?.find(({ id }) => id === groupId)
    return group ? { control, groupPaths: [group.path, ...group.memberPaths], workspace } : null
  }

  componentDidUpdate(_previous: unknown, _state: unknown, focus: FolderFocus | null) {
    if (!focus || focus.control.isConnected || document.activeElement !== document.body) return
    if ("groupPaths" in focus) {
      const savedRows = this.list.current?.querySelectorAll<HTMLElement>("[data-exact-folder]")
      for (const row of savedRows ?? []) {
        if (
          row.dataset.cloudWorkspace !== focus.workspace ||
          !row.dataset.exactFolder ||
          !focus.groupPaths.includes(row.dataset.exactFolder)
        )
          continue
        row.closest("details")?.querySelector("summary")?.focus({ preventScroll: true })
        return
      }
      return
    }
    const matching = new Set(
      this.props.groups
        ?.filter(
          (group) =>
            group.path === focus.exactPath || group.memberPaths.includes(focus.exactPath),
        )
        .map((group) => group.id),
    )
    const rows = this.list.current?.querySelectorAll<HTMLElement>("[data-cloud-folder]")
    for (const row of rows ?? []) {
      if (!row.dataset.cloudFolder || !matching.has(row.dataset.cloudFolder)) continue
      for (const target of row.querySelectorAll<HTMLElement>("[data-cloud-workspace]")) {
        if (target.dataset.cloudWorkspace !== focus.workspace) continue
        const control = target.querySelector<HTMLElement>(
          'select:not(:disabled), button[role="switch"]',
        )
        if (control) {
          control.focus({ preventScroll: true })
          return
        }
      }
    }
  }

  render() {
    return (
      <div ref={this.list} className="@container mt-3 flex flex-col divide-y divide-separator">
        {this.props.children}
      </div>
    )
  }
}
