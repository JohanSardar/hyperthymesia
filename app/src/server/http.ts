class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

async function request<T>(path: string, init: { method?: string; body?: unknown } = {}): Promise<T> {
  const response = await fetch(path, {
    method: init.method ?? "GET",
    credentials: "same-origin",
    headers: init.body === undefined ? undefined : { "Content-Type": "application/json" },
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  })
  if (response.status === 401) {
    window.location.assign("/login")
    throw new ApiError(401, "Signed out")
  }
  if (!response.ok) {
    let message = `The server answered ${response.status}`
    try {
      const data = (await response.json()) as { error?: string; message?: string }
      message = data.error ?? data.message ?? message
    } catch {
      // Not JSON; keep the status line.
    }
    throw new ApiError(response.status, message)
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}

export { ApiError, request }
