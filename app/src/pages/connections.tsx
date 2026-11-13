import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function ConnectionsPage() {
  return (
    <>
      <PageHeader title="Connections" />
      <Page width="explore">
        <Empty title="No connections yet" description="Your Mac is the first source. Other sources you connect show up here" />
      </Page>
    </>
  )
}

export { ConnectionsPage }
