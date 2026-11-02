/** Match the native opener scope for bundled promotion links. */
export function isAllowedPromotionUrl(url: URL): boolean {
  return (
    url.protocol === "https:" &&
    !url.username &&
    !url.password &&
    url.origin === "https://github.com" &&
    url.pathname.startsWith("/antiburn/antiburn/")
  )
}
