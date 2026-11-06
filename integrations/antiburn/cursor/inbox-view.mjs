import { formatSessionCheckUpdates } from '../../../packages/antiburn-plugin-core/session-checks-validation.mjs'

export function escapeInboxHtml(value) {
  return value.replace(
    /[&<>"']/g,
    (character) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#39;',
      })[character],
  )
}

function unescapeInlineMarkdown(text) {
  return text.replace(/\\([\\`*_[\]<>])/g, '$1')
}

/** Render a bounded inline subset. Links remain passive and raw HTML stays text. */
export function inboxInlineMarkdown(text) {
  const pattern =
    /\\([\\`*_[\]<>])|`([^`\n]+)`|\*\*((?:\\[^\n]|[^*\\\n])+)\*\*|\*((?:\\[^\n]|[^*\\\n])+)\*|\[([^\[\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g
  let html = ''
  let offset = 0
  for (const match of text.matchAll(pattern)) {
    html += escapeInboxHtml(text.slice(offset, match.index))
    html +=
      match[1] !== undefined
        ? escapeInboxHtml(match[1])
        : match[2] !== undefined
          ? `<code>${escapeInboxHtml(match[2])}</code>`
          : match[3] !== undefined
            ? `<strong>${escapeInboxHtml(unescapeInlineMarkdown(match[3]))}</strong>`
            : match[4] !== undefined
              ? `<em>${escapeInboxHtml(unescapeInlineMarkdown(match[4]))}</em>`
              : `${escapeInboxHtml(match[5])} (${escapeInboxHtml(match[6])})`
    offset = match.index + match[0].length
  }
  return html + escapeInboxHtml(text.slice(offset))
}

export function inboxFrameUpdates(result) {
  if (result?.sessionScope !== 'main') return []
  const findings =
    result.state === 'ready'
      ? result.snapshot
          .filter(
            (row) =>
              row.status === 'failure' &&
              row.eligibility === 'ready' &&
              typeof row.message === 'string' &&
              row.message.trim(),
          )
          .map((row) => ({ ...row, kind: 'finding' }))
      : []
  return [
    ...findings,
    ...result.updates.filter((row) => row.kind === 'cleared' || row.kind === 'invalidated'),
  ]
}

function cardHtml(card) {
  const title = `<h2>Conversation <code>${escapeInboxHtml(card.sessionId)}</code></h2>`
  if (card.unavailable)
    return `<section>${title}<p>Antiburn couldn’t load findings for this conversation.</p></section>`
  const result = card.result
  const frame = formatSessionCheckUpdates(inboxFrameUpdates(result))
  return `<section>${title}
    ${frame ? `<div class="authored">${inboxInlineMarkdown(frame)}</div>` : '<p>No finding is available for this conversation right now.</p>'}</section>`
}

/** The script confirms application only; it cannot invoke tools or load resources. */
export function renderCursorInbox(snapshot, nonce) {
  const confirmations = snapshot.cards
    .filter((card) => card.result?.receipt && inboxFrameUpdates(card.result).length)
    .map((card) => ({
      frame: snapshot.frame,
      sessionId: card.sessionId,
      receipt: card.result.receipt,
    }))
  const data = JSON.stringify(confirmations).replace(/</g, '\\u003c')
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}'; img-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'">
  <title>Antiburn Inbox</title><style nonce="${nonce}">
  body { font-family: var(--vscode-font-family); color: var(--vscode-editor-foreground); background: var(--vscode-editor-background); padding: 16px; line-height: 1.5; }
  section { border-top: 1px solid var(--vscode-panel-border); padding-block: 12px; }
  h1 { font-size: 1.3em; } h2, h3 { font-size: 1em; }
  code, pre { font-family: var(--vscode-editor-font-family); }
  pre, .authored { white-space: pre-wrap; overflow-wrap: anywhere; } code { overflow-wrap: anywhere; }
  </style></head><body><h1>antiburn Inbox</h1>
  <p>Antiburn findings from conversations across this workspace.</p>
  ${snapshot.cards.map(cardHtml).join('') || '<p>No findings to show yet.</p>'}
  <script nonce="${nonce}">
  const api = acquireVsCodeApi();
  const confirmations = ${data};
  function confirm() {
    if (document.visibilityState !== 'visible') return;
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (document.visibilityState === 'visible') for (const message of confirmations) api.postMessage(message);
    }));
  }
  document.addEventListener('visibilitychange', confirm);
  confirm();
  </script></body></html>`
}
