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
