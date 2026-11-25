import { afterEach, beforeEach, expect, it } from "vitest"
import { changeFolderWithFocus } from "./cloudFolderFocus"

let root: HTMLDivElement
let trigger: HTMLButtonElement
let folders: HTMLDivElement
let other: HTMLButtonElement

beforeEach(() => {
  root = document.createElement("div")
  trigger = document.createElement("button")
  folders = document.createElement("div")
  folders.id = "test-cloud-folders"
  folders.tabIndex = -1
  other = document.createElement("button")
  root.append(trigger, folders, other)
  document.body.append(root)
  trigger.focus()
})

afterEach(() => root.remove())

function disableTrigger() {
  // jsdom cannot blur an already disabled button. Move focus before disabling it.
  trigger.blur()
  trigger.disabled = true
}

it("restores a trigger that becomes usable after disabling removed focus", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    disableTrigger()
    trigger.disabled = false
  })
  expect(trigger).toHaveFocus()
})

it("focuses the list when the trigger stays disabled", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    disableTrigger()
  })
  expect(folders).toHaveFocus()
})

it("focuses the list when the trigger is removed", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    trigger.remove()
  })
  expect(folders).toHaveFocus()
})

it("does not restore an aria-disabled trigger", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    trigger.setAttribute("aria-disabled", "true")
    trigger.blur()
  })
  expect(folders).toHaveFocus()
})

it("keeps focus on another control the user selects during the action", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    disableTrigger()
    other.focus()
    trigger.disabled = false
  })
  expect(other).toHaveFocus()
})

it("does not restore focus after the user leaves the window", async () => {
  await changeFolderWithFocus(trigger, folders.id, undefined, async () => {
    disableTrigger()
    window.dispatchEvent(new Event("blur"))
    trigger.disabled = false
  })
  expect(document.body).toHaveFocus()
})
