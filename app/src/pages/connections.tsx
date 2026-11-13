import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function ConnectionsPage() {
  return (
    <>
      <PageHeader title="Connections" />
      <Page width="explore">
        <Empty title="No connections yet" description="Sources you connect beyond this Mac show up here" />
      </Page>
    </>
  )
}

export { ConnectionsPage }
