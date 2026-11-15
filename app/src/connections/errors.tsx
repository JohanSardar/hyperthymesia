/**
 * Plain words for a connection or pull error. The server's own text can be
 * technical; it stays one click away for anyone who wants it.
 */
function plainError(raw: string) {
  const text = raw.toLowerCase()
  if (/\b(401|403)\b|unauthori[sz]ed|forbidden|invalid (api )?(key|token)|bad credentials|permission denied/.test(text))
    return "The service refused the saved key or sign-in. Reconnect with one that works; nothing is pulled until then."
  if (/\b429\b|rate.?limit|too many requests/.test(text)) return "The service said too many requests. It tries again at the next pull."
  if (/timed? ?out|timeout|deadline/.test(text)) return "The service took too long to answer. It tries again at the next pull."
  if (/\b404\b|not found/.test(text) && !/command|executable/.test(text)) return "The address it pulls from doesn't exist any more. Running setup again lets the agent find the new one."
  if (/command not found|no such file|enoent|executable|not installed/.test(text)) return "The command it runs isn't installed on this computer, or isn't where it was. Install it, then run setup again."
  if (/dns|resolve|connection refused|unreachable|network|connect error|tls|certificate/.test(text))
    return "The service couldn't be reached. Check that it's online and the address is right; it tries again at the next pull."
  if (/\b5\d\d\b|server error|bad gateway|unavailable/.test(text)) return "The service had a problem on its side. It tries again at the next pull."
  if (/spending limit|budget/.test(text)) return "The monthly spending limit is reached, so setup waits until next month or until you raise it on the Agent page."
  if (/json|parse|schema|invalid response|unexpected/.test(text)) return "The service answered with something the pull couldn't read. Running setup again lets the agent adjust it."
  // The server's own words are often already plain; keep them over a vaguer line.
  if (raw.length <= 240 && /^[A-Z][^{}<>=_`\\]*[.!]$/.test(raw.trim())) return raw.trim()
  return "Something went wrong the last time it ran."
}

function PlainError({ text }: { text: string }) {
  return (
    <span className="plain-error">
      <span>{plainError(text)}</span>
      {plainError(text) !== text.trim() && (
        <details className="plain-error-details">
          <summary>Details</summary>
          <span className="mono-line plain-error-raw">{text}</span>
        </details>
      )}
    </span>
  )
}

export { PlainError, plainError }
