import { readdir, readFile, realpath } from "node:fs/promises"
import path from "node:path"

import type { Plugin } from "vite"

import { parsePromotionYaml } from "../src/promotions/validate"
import type { PromotionDefinition, PromotionMedia } from "../src/promotions/types"

const virtualModuleId = "virtual:promotions"
const resolvedVirtualModuleId = `\0${virtualModuleId}`
const promotionsDirectory = path.resolve(process.cwd(), "src/promotions")

type CatalogOptions = {
  directory?: string
  production: boolean
  developmentDirectory?: string
}

function isStillPoster(bytes: Buffer): boolean {
  if (bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) {
    for (let at = 8; at + 12 <= bytes.length;) {
      const size = bytes.readUInt32BE(at)
      const end = at + 12 + size
      if (end > bytes.length) return false
      const type = bytes.toString("ascii", at + 4, at + 8)
      if (type === "acTL") return false
      if (type === "IEND") return true
      at = end
    }
    return false
  }
  if (["GIF87a", "GIF89a"].includes(bytes.toString("ascii", 0, 6)) && bytes.length >= 13) {
    let at = 13 + (bytes.readUInt8(10) & 0x80 ? 3 * 2 ** ((bytes.readUInt8(10) & 7) + 1) : 0)
    let frames = 0
    while (at < bytes.length) {
      const block = bytes[at++]
      if (block === 0x3b) return frames === 1
      if (block === 0x2c) {
        if (at + 9 > bytes.length || ++frames > 1) return false
        const flags = bytes.readUInt8(at + 8)
        at += 9 + (flags & 0x80 ? 3 * 2 ** ((flags & 7) + 1) : 0)
        if (at >= bytes.length) return false
        at++
      } else if (block === 0x21) {
        if (at >= bytes.length) return false
        at++
      } else return false
      while (at < bytes.length && bytes[at] !== 0) at += 1 + bytes.readUInt8(at)
      if (at >= bytes.length) return false
      at++
    }
    return false
  }
  if (bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return true
  if (bytes.toString("ascii", 0, 4) === "RIFF" && bytes.toString("ascii", 8, 12) === "WEBP") {
    const end = bytes.readUInt32LE(4) + 8
    if (end > bytes.length || end < 20) return false
    for (let at = 12; at + 8 <= end;) {
      const type = bytes.toString("ascii", at, at + 4)
      const size = bytes.readUInt32LE(at + 4)
      const next = at + 8 + size + (size % 2)
      if (next > end) return false
      if (type === "ANIM" || type === "ANMF") return false
      if (type === "VP8X" && (size < 10 || (bytes.readUInt8(at + 8) & 0x02) !== 0)) return false
      at = next
    }
    return true
  }
  return false
}

async function collectMediaPaths(
  definition: PromotionDefinition,
  definitionDirectory: string,
): Promise<Map<string, string>> {
  const resolved = new Map<string, string>()
  const add = async (media: PromotionMedia | undefined) => {
    if (!media) return
    const variants = [media, ...(media.light ? [media.light] : [])]
    const sources = variants.flatMap((source) => [
      source.src,
      ...(source.poster ? [source.poster] : []),
    ])
    const realDirectory = await realpath(definitionDirectory)
    for (const source of sources) {
      const assetPath = path.resolve(definitionDirectory, source)
      let realAsset: string
      try {
        realAsset = await realpath(assetPath)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") {
          throw new Error(`Missing promotion asset "${source}" in ${definitionDirectory}`, {
            cause: error,
          })
        }
        throw error
      }
      const relative = path.relative(realDirectory, realAsset)
      if (
        relative === ".." ||
        relative.startsWith(`..${path.sep}`) ||
        path.isAbsolute(relative)
      ) {
        throw new Error(`Promotion asset "${source}" resolves outside ${definitionDirectory}`)
      }
      if (variants.some((variant) => variant.poster === source)) {
        const bytes = await readFile(realAsset)
        if (!isStillPoster(bytes)) {
          throw new Error(
            `Promotion poster "${source}" must be a still PNG, JPEG, GIF, or WebP image`,
          )
        }
      }
      resolved.set(source, realAsset)
    }
  }
  await add(definition.screens.announcement.media)
  return resolved
}

export async function generatePromotionModule({
  directory = promotionsDirectory,
  production,
  developmentDirectory,
}: CatalogOptions): Promise<{ code: string; watchFiles: string[]; ids: string[] }> {
  const directories = [
    directory,
    ...(!production && developmentDirectory ? [developmentDirectory] : []),
  ]
  const entries: Array<{ definition: PromotionDefinition; folder: string }> = []
  const watchFiles: string[] = []
  const ids = new Set<string>()

  for (const sourceDirectory of directories) {
    const folders = (await readdir(sourceDirectory, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort()
    for (const folder of folders) {
      const folderPath = path.join(sourceDirectory, folder)
      const yamlPath = path.join(folderPath, "promo.yaml")
      let source: string
      try {
        source = await readFile(yamlPath, "utf8")
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") continue
        throw error
      }
      watchFiles.push(yamlPath)
      const definition = parsePromotionYaml(source, yamlPath)
      if (ids.has(definition.id))
        throw new Error(`Duplicate promotion id "${definition.id}" in ${yamlPath}`)
      ids.add(definition.id)
      if (production && definition.development_only) continue
      entries.push({ definition, folder: folderPath })
    }
  }

  const imports: string[] = []
  const definitions: string[] = []
  const names: string[] = []
  for (const { definition, folder } of entries) {
    const mediaPaths = await collectMediaPaths(definition, folder)
    const replacements = new Map<string, string>()
    for (const [source, assetPath] of mediaPaths) {
      const variable = `__promotionAsset${imports.length}`
      imports.push(`import ${variable} from ${JSON.stringify(`${assetPath}?no-inline`)};`)
      replacements.set(source, variable)
      watchFiles.push(assetPath)
    }
    const name = `__promotion${names.length}`
    names.push(name)
    definitions.push(`const ${name} = ${JSON.stringify(definition)};`)
    const media = definition.screens.announcement.media
    if (media) {
      definitions.push(
        `${name}.screens.announcement.media.src = ${replacements.get(media.src)};`,
      )
      if (media.poster)
        definitions.push(
          `${name}.screens.announcement.media.poster = ${replacements.get(media.poster)};`,
        )
      if (media.light) {
        definitions.push(
          `${name}.screens.announcement.media.light.src = ${replacements.get(media.light.src)};`,
        )
        if (media.light.poster)
          definitions.push(
            `${name}.screens.announcement.media.light.poster = ${replacements.get(media.light.poster)};`,
          )
      }
    }
  }

  return {
    code: `${imports.join("\n")}\n${definitions.join("\n")}\nexport default [${names.join(",")}];\n`,
    watchFiles,
    ids: entries.map(({ definition }) => definition.id),
  }
}

export function promotionCatalogPlugin(
  command: "build" | "serve",
  mode: string,
  options: { developmentDirectory?: string } = {},
): Plugin {
  const production = command === "build" && mode === "production"
  return {
    name: "antiburn-promotion-catalog",
    resolveId(id) {
      return id === virtualModuleId ? resolvedVirtualModuleId : null
    },
    async load(id) {
      if (id !== resolvedVirtualModuleId) return null
      const module = await generatePromotionModule({ production, ...options })
      for (const watchFile of module.watchFiles) this.addWatchFile(watchFile)
      return module.code
    },
    configureServer(server) {
      const directories = [
        promotionsDirectory,
        ...(options.developmentDirectory ? [options.developmentDirectory] : []),
      ]
      server.watcher.add(directories)
      server.watcher.on("all", (event, changedPath) => {
        if (event !== "add" && event !== "change" && event !== "unlink") return
        if (
          !directories.some((directory) => {
            const relative = path.relative(directory, changedPath)
            return (
              relative !== "" &&
              relative !== ".." &&
              !relative.startsWith(`..${path.sep}`) &&
              !path.isAbsolute(relative)
            )
          })
        )
          return
        const module = server.moduleGraph.getModuleById(resolvedVirtualModuleId)
        if (!module) return
        server.moduleGraph.invalidateModule(module)
        server.ws.send({ type: "full-reload" })
      })
    },
  }
}
