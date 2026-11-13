import { PageHeader } from "../shell"
import { Empty, Page } from "../ui"

function TodayPage() {
  return (
    <>
      <PageHeader title="Today" />
      <Page width="dashboard">
        <Empty title="No entries yet" description="Entries show up here the moment they're recorded" />
      </Page>
    </>
  )
}

export { TodayPage }
