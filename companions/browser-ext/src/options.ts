const input = document.querySelector<HTMLTextAreaElement>("#patterns")!;
void chrome.storage.local.get(["excludedPatterns"]).then(v => { input.value = (v.excludedPatterns ?? []).join("\n"); });
document.querySelector("#save")!.addEventListener("click", async () => { await chrome.storage.local.set({excludedPatterns: input.value.split("\n").map(x => x.trim()).filter(Boolean)}); document.querySelector("#status")!.textContent = "Saved"; });
