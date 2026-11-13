import "@fontsource-variable/inter"
import "@fontsource-variable/geist-mono"
import "./styles/tokens.css"
import "./styles/base.css"
import "./styles/components.css"
import "./styles/shell.css"
import "./theme"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { App } from "./app"

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
