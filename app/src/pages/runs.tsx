import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function RunsPage() {
  return (
    <>
      <PageHeader title="Runs" />
      <Page width="dashboard">
        <Empty title="No runs yet" description="Passes run every 4 hours. Each pass and pull shows up here" />
      </Page>
    </>
  )
}

export { RunsPage }
