import { Component, createRef, type ReactNode } from "react"
import type { CloudProjectGroup } from "../../lib/cloud"

interface FolderFocus {
  control: Element
  path: string
  workspace: string
}

/** Keep focus on the same saved permission when discovery replaces an exact row. */
export class CloudFolderList extends Component<{
  children: ReactNode
  groups: readonly CloudProjectGroup[] | undefined
}> {
  private list = createRef<HTMLDivElement>()

  getSnapshotBeforeUpdate(): FolderFocus | null {
    const control = document.activeElement
    if (
      !document.hasFocus() ||
      !control?.matches('[role="switch"]') ||
      !this.list.current?.contains(control)
    )
      return null
    const row = control.closest<HTMLElement>("[data-exact-folder]")
    const path = row?.dataset.exactFolder
    const workspace = row?.dataset.cloudWorkspace
    return path && workspace ? { control, path, workspace } : null
  }

  componentDidUpdate(_previous: unknown, _state: unknown, focus: FolderFocus | null) {
    if (!focus || focus.control.isConnected || document.activeElement !== document.body) return
    const matching = new Set(
      this.props.groups
        ?.filter((group) => group.path === focus.path || group.memberPaths.includes(focus.path))
        .map((group) => group.id),
    )
    const rows = this.list.current?.querySelectorAll<HTMLElement>("[data-cloud-folder]")
    for (const row of rows ?? []) {
      if (!row.dataset.cloudFolder || !matching.has(row.dataset.cloudFolder)) continue
      for (const target of row.querySelectorAll<HTMLElement>("[data-cloud-workspace]")) {
        if (target.dataset.cloudWorkspace !== focus.workspace) continue
        const control = target.querySelector<HTMLElement>(
          'button[role="switch"]:not(:disabled)',
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
