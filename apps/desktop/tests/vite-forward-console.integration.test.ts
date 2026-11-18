import { mkdtemp, rm } from "node:fs/promises"
import os from "node:os"
import path from "node:path"

import { createLogger, createServer, createServerHotChannel, DevEnvironment } from "vite"
import { expect, it, vi } from "vitest"

it.each(["error", "unhandled-rejection"])(
  "forwards a Safari %s with a file-only module without stopping Vite",
  async (type) => {
    const root = await mkdtemp(path.join(os.tmpdir(), "antiburn-vite-console-"))
    const logger = createLogger("silent")
    const error = vi.spyOn(logger, "error")
    const transport = createServerHotChannel()
    const server = await createServer({
      root,
      configFile: false,
      customLogger: logger,
      optimizeDeps: { noDiscovery: true },
      server: {
        middlewareMode: true,
        ws: false,
        forwardConsole: true,
        watch: null,
      },
      environments: {
        client: {
          dev: {
            createEnvironment: (name, config) =>
              new DevEnvironment(name, config, { hot: true, transport }),
          },
        },
      },
    })

    try {
      const module = server.environments.client.moduleGraph.createFileOnlyEntry(
        path.join(server.config.root, "main.html"),
      )
      expect(module.id).toBeNull()

      if (!transport.api) throw new Error("Missing test transport")
      const { innerEmitter } = transport.api
      expect(() =>
        innerEmitter.emit(
          "vite:forward-console",
          {
            type,
            data: {
              name: "Error",
              message: "Synthetic renderer failure",
              stack: "global code@http://127.0.0.1:1420/main.html:1:1",
            },
          },
          { send: vi.fn() },
        ),
      ).not.toThrow()

      expect(error).toHaveBeenCalledExactlyOnceWith(
        expect.stringContaining("Synthetic renderer failure"),
        expect.objectContaining({ timestamp: true }),
      )
      expect(error.mock.calls[0][0]).toContain("main.html:1:1")
      expect(
        await server.transformIndexHtml("/main.html", "<h1>Synthetic renderer</h1>"),
      ).toContain("Synthetic renderer")
    } finally {
      await server.close()
      await rm(root, { recursive: true, force: true })
    }
  },
)
