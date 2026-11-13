import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function AgentPage() {
  return (
    <>
      <PageHeader title="Agent" />
      <Page width="settings">
        <Empty title="Nothing set up yet" description="When you add a connection, what the agent set up for it shows here" />
      </Page>
    </>
  )
}

export { AgentPage }
