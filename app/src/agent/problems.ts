import type { ProviderId } from "../server/api"
import { provider } from "../model/providers"

/** What the fix button does. */
type Fix = "signin" | "key" | "model" | "limit" | "rerun" | null

type Problem = { title: string; body: string; fix: Fix; fixLabel?: string }

/**
 * Plain words and one fix for each problem code the server records. The
 * server never keeps provider text, so these are the whole story.
 */
function describeProblem(code: string | null, providerId: ProviderId | null, kind: "pass" | "setup" | "check" = "pass"): Problem | null {
  if (!code) return null
  const name = providerId ? provider(providerId)?.name ?? "your provider" : "your provider"
  const nothing = kind === "setup" ? "The connection wasn't set up." : kind === "check" ? "The test didn't get an answer." : "Nothing was written for this window."
  switch (code) {
    case "signin":
      return {
        title: `The ${name} sign-in stopped working`,
        body: `${name} didn't accept the saved login. ${nothing} Sign in again and it runs again.`,
        fix: "signin",
        fixLabel: "Sign in again",
      }
    case "key":
      return { title: "The API key didn't work", body: `${name} refused the saved key. ${nothing} Add a working key and run it again.`, fix: "key", fixLabel: "Add a new key" }
    case "model":
      return {
        title: "The model isn't available",
        body: `${name} doesn't offer this model to your account any more. ${nothing} Pick another model.`,
        fix: "model",
        fixLabel: "Choose a model",
      }
    case "no_model":
      return { title: "No model is chosen", body: "Nothing runs until you choose a provider and model.", fix: "model", fixLabel: "Choose a model" }
    case "usage_limit":
      return {
        title: "Your plan's limit is reached",
        body: `${name} says this plan has used its allowance for now. It tries again by itself in about 30 minutes.`,
        fix: "rerun",
        fixLabel: "Try now",
      }
    case "spending_limit":
      return {
        title: "This month's spending limit is reached",
        body: "No new runs start until next month, or until you raise or remove the limit. Runs already going finished.",
        fix: "limit",
        fixLabel: "Change the limit",
      }
    case "timeout":
      return { title: "It took too long", body: `The model didn't finish within 20 minutes, so the run was stopped. ${nothing}`, fix: "rerun", fixLabel: "Run again" }
    case "too_long":
      return { title: "It needed too many steps", body: `The model kept looking things up past its step budget. ${nothing}`, fix: "rerun", fixLabel: "Run again" }
    case "output":
      return { title: "The answer couldn't be used", body: `The model's answer wasn't in the shape a day page needs. ${nothing}`, fix: "rerun", fixLabel: "Run again" }
    case "superseded":
      return { title: "New activity came in", body: "More was recorded for this window while it ran, so it starts over with everything. Nothing to do.", fix: null }
    case "busy":
      return { title: "Waiting for another run", body: `Another run is using the ${name} login. This one starts right after.`, fix: null }
    case "waiting":
      return { title: "Waiting for the window to settle", body: "It runs once the last few minutes of recording are in.", fix: null }
    default:
      return { title: "Something went wrong", body: `${nothing} The server log has the details.`, fix: "rerun", fixLabel: "Run again" }
  }
}

export { describeProblem }
export type { Fix, Problem }
