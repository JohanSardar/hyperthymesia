export function cx(...names: (string | false | null | undefined)[]) {
  return names.filter(Boolean).join(" ")
}
