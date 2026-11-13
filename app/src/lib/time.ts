const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" })
const dayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" })

function clock(iso: string) {
  return timeFormat.format(new Date(iso))
}

/** "just now", "4 min ago", "3 h ago", then a date. */
function ago(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "never"
  const seconds = Math.round((now - new Date(iso).getTime()) / 1000)
  if (seconds < 45) return "just now"
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  return days < 7 ? `${days} d ago` : dayFormat.format(new Date(iso))
}

/** "in 3 h", "in 12 min", "due now". */
function until(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "not scheduled"
  const seconds = Math.round((new Date(iso).getTime() - now) / 1000)
  if (seconds <= 30) return "due now"
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `in ${minutes} min`
  const hours = Math.round(minutes / 60)
  return hours < 48 ? `in ${hours} h` : `on ${dayFormat.format(new Date(iso))}`
}

export { ago, clock, until }
