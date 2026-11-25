import { useId, useRef, useState } from "react"
import {
  CLOUD_BUSY_MESSAGE,
  type CloudTeamJoinAction,
  type CloudTeamJoinResult,
} from "../lib/cloud"
import { isCloudBusy } from "./cloudSessionState"

const focusInput = (node: HTMLInputElement | null) => node?.focus()
const focusButton = (node: HTMLButtonElement | null) => node?.focus()
const focusReview = (node: HTMLParagraphElement | null) => node?.focus()

const joinIssues = {
  billingInactive: "This Team's subscription is inactive. Ask a Team admin to check billing.",
  billingStale: "Cloud could not check this Team's seats. Try again.",
  full: "This Team has no seats available. Ask a Team admin for a seat, then try again.",
}

type JoinStep = Exclude<
  CloudTeamJoinResult,
  { status: "billingInactive" | "billingStale" | "full" }
>

function joinAction(step: JoinStep | undefined, input: string): CloudTeamJoinAction | null {
  if (step?.status === "ready")
    return { action: "claim", attemptId: step.attemptId, emailId: step.emailId }
  if (step?.status === "started" || step?.status === "verificationPending")
    return { action: "resume", attemptId: step.attemptId }
  const value = input.trim()
  if (!value || step?.status === "joined" || step?.status === "removed") return null
  if (step?.status === "needsEmail")
    return { action: "selectEmail", attemptId: step.attemptId, address: value }
  return { action: "start", link: value }
}

function joinErrorMessage(error: unknown) {
  if (isCloudBusy(error)) return CLOUD_BUSY_MESSAGE
  switch (error instanceof Error ? error.message : error) {
    case "team_join_invalid_invite":
      return "Paste the full invite link from your Team admin."
    case "team_join_inactive_invite":
      return "This invite or join attempt is no longer active. Check the current invite link."
    case "team_join_invalid_email":
      return "Check your work email address and try again."
    case "team_join_reapproval_required":
      return "Sign out and sign in again to approve Team joining on this device."
    case "team_join_rate_limited":
      return "Wait a minute before trying again."
    case "team_join_unavailable":
      return "Team joining is unavailable for this connection."
    case "team_join_account_changed":
    case "team_join_sign_in_required":
      return "Sign in to Cloud again before joining a Team."
    default:
      return "Could not confirm the Team join request. Try again."
  }
}

/** Keep pasted links and the active join attempt in this view only. */
export function CloudTeamJoin({
  send,
  onboarding = false,
}: {
  send: (action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>
  onboarding?: boolean
}) {
  const id = useId()
  const openButton = useRef<HTMLButtonElement>(null)
  const inFlight = useRef(false)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [step, setStep] = useState<JoinStep>()
  const finished = step?.status === "joined" || step?.status === "removed"
  const [input, setInput] = useState("")
  const [message, setMessage] = useState("")
  const [reviewEmail, setReviewEmail] = useState(false)
  const buttonClass = onboarding ? "onboarding-secondary" : "ui-push-button"
  const fieldClass =
    "w-full min-w-0 rounded-control border border-separator bg-input-fill px-3 py-2 type-callout text-label"

  const close = () => {
    setOpen(false)
    setStep(undefined)
    setInput("")
    setMessage("")
    setReviewEmail(false)
    openButton.current?.focus()
  }

  const sendAction = async (action: CloudTeamJoinAction) => {
    if (inFlight.current) return
    setMessage("")
    inFlight.current = true
    setBusy(true)
    let started = step !== undefined
    try {
      let result = await send(action)
      if (result.status === "started") {
        started = true
        setStep({ status: "started", attemptId: result.attemptId })
        setInput("")
        result = await send({ action: "resume", attemptId: result.attemptId })
      }
      switch (result.status) {
        case "started":
          setMessage(joinErrorMessage(undefined))
          break
        case "billingInactive":
        case "billingStale":
        case "full":
          setMessage(joinIssues[result.status])
          break
        default:
          setReviewEmail(
            result.status === "ready" &&
              step?.status === "ready" &&
              (step.emailId !== result.emailId || step.email !== result.email),
          )
          setStep(result)
          setInput(result.status === "needsEmail" ? (result.email ?? "") : "")
      }
    } catch (error) {
      setMessage(joinErrorMessage(error))
      if (error === "team_join_inactive_invite" && started) {
        setStep(undefined)
        setInput("")
        setReviewEmail(false)
      }
    } finally {
      inFlight.current = false
      setBusy(false)
    }
  }

  const submit = (kind: "continue" | "claim") => {
    const action = joinAction(step, input)
    if (action && (action.action === "claim") === (kind === "claim")) void sendAction(action)
  }

  return (
    <section
      className="flex min-w-0 flex-col items-start gap-2 text-left"
      aria-label="Team join"
      onKeyDownCapture={(event) => {
        if (event.key === "Enter" && event.repeat) event.preventDefault()
      }}
    >
      <button
        className={buttonClass}
        type="button"
        ref={openButton}
        aria-disabled={busy || undefined}
        aria-expanded={open}
        aria-controls={`${id}-form`}
        onClick={() => {
          if (!busy) {
            if (open) close()
            else setOpen(true)
          }
        }}
      >
        {open ? "Hide Team invite" : "Join a Team"}
      </button>
      {open && (
        <form
          id={`${id}-form`}
          className="flex w-full min-w-0 flex-col gap-2"
          aria-busy={busy}
          onSubmit={(event) => {
            event.preventDefault()
            submit("continue")
          }}
        >
          {step?.status === "removed" ? (
            <p className="type-callout" role="status">
              An admin removed you from this workspace.
            </p>
          ) : step?.status === "joined" ? (
            <p className="wrap-anywhere type-callout" role="status">
              You joined {step.teamName}.
              {step.inSetup ? " The owner still needs to open the Team." : ""}
            </p>
          ) : step?.status === "ready" ? (
            <p
              key={`${step.emailId}:${step.email}`}
              className="wrap-anywhere type-callout"
              tabIndex={reviewEmail ? -1 : undefined}
              ref={reviewEmail ? focusReview : undefined}
            >
              {reviewEmail ? "The selected email changed. Review it before joining. " : ""}
              Join {step.teamName} with {step.email}. Joining does not start uploads.
            </p>
          ) : step?.status === "verificationPending" ? (
            <p className="wrap-anywhere type-footnote">
              Open the link sent to {step.email} and verify it with the same Cloud account in
              your browser. Return here to check verification and join {step.teamName}. If your
              link expired, request another email.
            </p>
          ) : step?.status !== "started" ? (
            <>
              {step && (
                <p className="wrap-anywhere type-footnote">
                  Choose the work email you want to use for {step.teamName}.
                </p>
              )}
              <label htmlFor={`${id}-input`} className="type-callout font-medium">
                {step ? "Work email" : "Team invite link"}
              </label>
              <input
                key={step?.status ?? "invite"}
                id={`${id}-input`}
                ref={focusInput}
                className={fieldClass}
                type={step?.status === "needsEmail" ? "email" : "text"}
                autoComplete={step?.status === "needsEmail" ? "email" : "off"}
                spellCheck={false}
                required
                maxLength={step?.status === "needsEmail" ? 254 : 4096}
                value={input}
                onChange={(event) => setInput(event.currentTarget.value)}
                readOnly={busy}
              />
            </>
          ) : null}
          {message && (
            <p className="wrap-anywhere type-footnote" role="alert">
              {message}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            {!finished && (
              <button
                key={step?.status ?? "invite"}
                type={step?.status === "ready" ? "button" : "submit"}
                onClick={step?.status === "ready" ? () => submit("claim") : undefined}
                ref={
                  step && step.status !== "needsEmail" && !reviewEmail ? focusButton : undefined
                }
                className={buttonClass}
                aria-disabled={busy || undefined}
              >
                {busy
                  ? "Working…"
                  : step?.status === "needsEmail"
                    ? "Use this email"
                    : step?.status === "verificationPending"
                      ? "Check verification"
                      : step?.status === "ready"
                        ? "Join Team"
                        : "Check invite"}
              </button>
            )}
            {step?.status === "verificationPending" && (
              <>
                <button
                  type="button"
                  className={buttonClass}
                  disabled={busy}
                  onClick={() => {
                    setMessage("")
                    setInput(step.email)
                    setStep({
                      status: "needsEmail",
                      attemptId: step.attemptId,
                      teamName: step.teamName,
                    })
                  }}
                >
                  Change email
                </button>
                <button
                  type="button"
                  className={buttonClass}
                  disabled={busy}
                  onClick={() =>
                    void sendAction({
                      action: "selectEmail",
                      attemptId: step.attemptId,
                      address: step.email,
                      resend: true,
                    })
                  }
                >
                  Request another email
                </button>
              </>
            )}
            <button
              type="button"
              ref={finished ? focusButton : undefined}
              className={buttonClass}
              disabled={busy}
              onClick={close}
            >
              {finished ? "Done" : "Cancel join"}
            </button>
          </div>
        </form>
      )}
    </section>
  )
}
