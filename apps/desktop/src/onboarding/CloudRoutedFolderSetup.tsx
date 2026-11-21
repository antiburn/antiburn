import { CloudFolderSetup } from "./CloudFolderSetup"
import type { CloudSession, CloudViewSnapshot } from "./CloudSession"

/** Share the same draft and retry flow in onboarding and Settings. */
export function CloudRoutedFolderSetup({
  session,
  value,
  busy = false,
  error,
  onFinish,
  onBack,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  busy?: boolean
  error?: string | null | undefined
  onFinish: () => void
  /** Settings omits Back and keeps the finished form open. */
  onBack?: () => void
}) {
  const { cloud, projects, routedFolders } = value
  const message =
    error ??
    value.error ??
    cloud.setupError ??
    projects?.error ??
    routedFolders?.message ??
    (cloud.destinations === null
      ? "Could not check Cloud destinations."
      : projects?.partial && !projects.hasMore
        ? "Some local folders could not be checked."
        : null)
  return (
    <CloudFolderSetup
      folders={projects?.groups ?? []}
      destinations={cloud.destinations ?? []}
      destinationsKnown={Array.isArray(cloud.destinations)}
      choices={routedFolders?.choices ?? {}}
      lockedChoices={routedFolders?.locked ?? {}}
      homeDirectory={value.homeDirectory}
      loading={!!projects?.loading || (!projects && !!cloud.destinations?.length && !message)}
      busy={busy || value.busy}
      choicesBlocked={!routedFolders?.ready}
      startBlocked={
        !routedFolders?.ready ||
        !cloud.captureEnabled ||
        cloud.status !== "signed_in" ||
        !!(error || value.error || cloud.setupError || projects?.error)
      }
      message={message}
      results={routedFolders?.results ?? []}
      onLoadMore={projects?.hasMore ? () => void session.loadMoreProjects() : undefined}
      onReview={
        routedFolders?.retry || projects?.error || cloud.destinations === null
          ? () =>
              void (cloud.destinations === null ? session.load() : session.reviewFolderRoutes())
          : undefined
      }
      onRetry={routedFolders?.retry ? () => void session.retryFolderRoutes() : undefined}
      onChoiceChange={session.selectFolderRoute}
      onStart={(assignments) => void session.startFolderRoutes(value, assignments, !!onBack)}
      onFinish={() => {
        session.dismissFolderRoutes()
        onFinish()
      }}
      onBack={onBack}
    />
  )
}
