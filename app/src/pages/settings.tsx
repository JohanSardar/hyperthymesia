import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function SettingsPage() {
  return (
    <>
      <PageHeader title="Settings" />
      <Page width="settings">
        <Empty title="Nothing to change yet" description="Model, connections, recording, storage, API keys and appearance live here" />
      </Page>
    </>
  )
}

export { SettingsPage }
