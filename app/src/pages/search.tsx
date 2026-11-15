import { Placeholder } from "../shell"
import { Kbd } from "../ui"

function SearchPage() {
  return (
    <Placeholder
      path="/search"
      title="Nothing to search yet"
      description={
        <>
          Searching what you've seen will happen here. For now, press <Kbd>⌘K</Kbd> to jump to any page.
        </>
      }
    />
  )
}

export { SearchPage }
