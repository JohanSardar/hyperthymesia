import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function RunsPage() {
  return (
    <>
      <PageHeader title="Runs" />
      <Page width="dashboard">
        <Empty title="No runs yet" description="Each 4-hour pass and every pull shows up here" />
      </Page>
    </>
  )
}

export { RunsPage }
