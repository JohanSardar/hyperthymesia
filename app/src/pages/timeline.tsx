import { Link } from "react-router"
import { Placeholder } from "../shell"
import { buttonAttrs } from "../ui"

function TimelinePage() {
  return (
    <Placeholder
      path="/timeline"
      title="No timeline yet"
      description="Your days will line up here hour by hour. Until then, Today shows what arrives."
      action={
        <Link to="/today" {...buttonAttrs("default", "sm")}>
          Open Today
        </Link>
      }
    />
  )
}

export { TimelinePage }
