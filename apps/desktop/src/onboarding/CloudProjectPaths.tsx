/** Let keyboard and pointer users inspect the exact current folder members.
 * This disclosure does not select folders, discover paths, or grant consent.
 */
export function CloudProjectPaths({ name, paths }: { name: string; paths: readonly string[] }) {
  if (!paths.length) return null
  return (
    <details className="mt-1 min-w-0 type-footnote">
      <summary className="cursor-pointer" aria-label={`Included folders for ${name}`}>
        Included folders
      </summary>
      <ul className="mt-2 space-y-1" role="list">
        {paths.map((path) => (
          <li key={path} className="break-all">
            {path}
          </li>
        ))}
      </ul>
    </details>
  )
}
