let timer: ReturnType<typeof setTimeout>;
function schedule() { clearTimeout(timer); timer = setTimeout(() => { void capture(); }, 250); }
async function capture() {
  try {
    const url = location.href, pageVisible = document.visibilityState === "visible";
    await chrome.runtime.sendMessage({type:"visibility", url, pageVisible});
    if (!pageVisible || !(await chrome.runtime.sendMessage({type:"policy"}))?.allowed) return;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    let text = "", count = 0, truncated = false;
    while (walker.nextNode()) {
      if (++count > 5000) { truncated = true; break; }
      const node = walker.currentNode;
      const parent = node.parentElement;
      if (!parent || parent.closest("script,style,noscript,input,textarea,[contenteditable],[hidden],[aria-hidden=true]")) continue;
      const style = getComputedStyle(parent);
      if (style.visibility !== "visible" || style.display === "none" || Number(style.opacity) === 0) continue;
      const range = document.createRange(); range.selectNodeContents(node);
      if (!Array.from(range.getClientRects()).some(r => r.width > 0 && r.height > 0 && r.top < innerHeight && r.bottom > 0 && r.left < innerWidth && r.right > 0)) continue;
      const value = node.textContent?.trim(); if (!value) continue;
      text += value + "\n";
      if (text.length > 16000) { text = text.slice(0,16000); truncated = true; break; }
    }
    if (location.href !== url || document.visibilityState !== "visible") return;
    await chrome.runtime.sendMessage({type:"snapshot", url, pageVisible, text, truncated});
  } catch { /* Extension can be reloaded while this document stays open. */ }
}
document.addEventListener("visibilitychange", schedule);
window.addEventListener("popstate", schedule);
chrome.runtime.onMessage.addListener(msg => { if (msg?.type === "route") schedule(); });
schedule();
