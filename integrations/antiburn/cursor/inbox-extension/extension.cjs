const vscode = require('vscode')
const { mkdirSync, realpathSync } = require('node:fs')
const { randomBytes } = require('node:crypto')
const { join, resolve } = require('node:path')

/** Register opt-in UI commands. Activation does not enroll a workspace. */
async function activate(context) {
  const { enrollInbox, renewInbox, disposeInboxEnrollment, readInboxReferences } =
    await import('../inbox-bridge.mjs')
  const { CursorInboxController } = await import('../inbox-controller.mjs')
  const { renderCursorInbox } = await import('../inbox-view.mjs')
  let enrollment
  let controller
  let panel
  let timer
  let ticking = false
  let lastRenewedAt = 0
  const output = vscode.window.createOutputChannel('Antiburn Inbox configuration')
  context.subscriptions.push(output)

  function render() {
    if (panel && controller && enrollment)
      panel.webview.html = renderCursorInbox(
        controller.snapshot(),
        randomBytes(16).toString('hex'),
      )
  }

  function show() {
    if (!enrollment || !controller) return
    controller.resume()
    if (panel) {
      panel.reveal(vscode.ViewColumn.Beside, true)
      controller.setVisible(panel.visible)
      render()
      return
    }
    panel = vscode.window.createWebviewPanel(
      'antiburnCursorInbox',
      'Antiburn Inbox',
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, enableCommandUris: false, localResourceRoots: [] },
    )
    const currentPanel = panel
    const currentController = controller
    currentController.setVisible(panel.visible)
    const received = panel.webview.onDidReceiveMessage((message) => {
      if (panel !== currentPanel || controller !== currentController || !currentPanel.visible)
        return
      void currentController.confirmApplied(message)
    })
    const visibility = panel.onDidChangeViewState(() => {
      if (panel !== currentPanel || controller !== currentController) return
      currentController.setVisible(currentPanel.visible)
      if (currentPanel.visible) render()
    })
    panel.onDidDispose(() => {
      received.dispose()
      visibility.dispose()
      currentController.close()
      if (panel === currentPanel) panel = undefined
    })
    render()
  }

  function stop() {
    if (timer) clearInterval(timer)
    timer = undefined
    controller?.dispose()
    controller = undefined
    panel?.dispose()
    panel = undefined
    if (enrollment) disposeInboxEnrollment(enrollment)
    enrollment = undefined
  }

  async function tick() {
    if (ticking || !controller || !enrollment) return
    const current = controller
    const enrolled = enrollment
    ticking = true
    try {
      if (Date.now() - lastRenewedAt > 20_000) {
        renewInbox(enrolled)
        lastRenewedAt = Date.now()
      }
      await current.poll(readInboxReferences(enrolled))
    } catch {
      if (controller === current) stop()
    } finally {
      ticking = false
    }
  }

  async function enroll() {
    if (enrollment) {
      show()
      return
    }
    const folders = vscode.workspace.workspaceFolders
    if (
      process.platform === 'win32' ||
      vscode.env.remoteName ||
      !folders?.length ||
      folders.length > 8 ||
      folders.some((folder) => folder.uri.scheme !== 'file')
    ) {
      void vscode.window.showInformationMessage(
        'Antiburn Inbox requires an explicitly enrolled local POSIX workspace with at most eight roots.',
      )
      return
    }
    try {
      mkdirSync(context.globalStorageUri.fsPath, {
        recursive: true,
        mode: 0o700,
      })
      const base = join(realpathSync(context.globalStorageUri.fsPath), 'inbox-v1')
      enrollment = enrollInbox(
        base,
        folders.map((folder) => folder.uri.fsPath),
      )
      controller = new CursorInboxController({
        changed: (_snapshot, options) => {
          if (options.reveal) show()
          else render()
        },
      })
      lastRenewedAt = Date.now()
      show()
      timer = setInterval(() => {
        void tick()
      }, 5000)
      void tick()
    } catch {
      stop()
      void vscode.window.showInformationMessage(
        'Antiburn Inbox could not create a private enrollment. No hook configuration was changed.',
      )
    }
  }

  async function previewConfig() {
    if (!enrollment) {
      void vscode.window.showInformationMessage(
        'Enroll this workspace window before previewing its optional hook configuration.',
      )
      return
    }
    const confirmed = await vscode.window.showInformationMessage(
      'Preview the separate optional reference hook? This does not change Cursor configuration.',
      'Preview',
    )
    if (confirmed !== 'Preview' || !enrollment) return
    const hook = resolve(__dirname, '../inbox-reference-hook.mjs')
    const quote = (text) => `'${text.replace(/'/g, "'\\''")}'`
    output.replace(
      JSON.stringify(
        {
          version: 1,
          hooks: {
            postToolUse: [
              {
                command: `node ${quote(hook)} --inbox-dir ${quote(enrollment.base)}`,
              },
            ],
          },
        },
        null,
        2,
      ),
    )
    output.show(true)
  }

  context.subscriptions.push(
    vscode.commands.registerCommand('antiburn.cursorInbox.enroll', enroll),
    vscode.commands.registerCommand('antiburn.cursorInbox.show', show),
    vscode.commands.registerCommand('antiburn.cursorInbox.hookConfig', previewConfig),
    vscode.commands.registerCommand('antiburn.cursorInbox.stop', stop),
    vscode.workspace.onDidChangeWorkspaceFolders(stop),
    { dispose: stop },
  )
}

module.exports = { activate }
