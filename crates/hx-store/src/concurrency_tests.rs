use super::*;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Duration as Wait, timeout};

// Re-execute just this helper so SIGKILL tests the actual Store guard in a
// different process. The parent owns the synthetic database and its cleanup.
#[tokio::test]
#[ignore = "subprocess helper for interpretation_lock_releases_on_process_death_and_drop"]
async fn interpretation_lock_child() {
    let dir = std::env::var("HX_LOCK_TEST_DIR").unwrap();
    let store = Store::open(dir).await.unwrap();
    let _guard = store
        .try_interpretation_guard(window_at(DateTime::UNIX_EPOCH))
        .await
        .unwrap()
        .unwrap();
    println!("LOCKED");
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn interpretation_lock_releases_on_process_death_and_drop() {
    let f = test_support::Fixture::new().await;
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "concurrency_tests::interpretation_lock_child",
            "--ignored",
            "--nocapture",
            // Serial libtest prefixes stdout with its progress label, which
            // would hide the standalone LOCKED protocol line from the parent.
            "--test-threads=2",
        ])
        .env("HX_LOCK_TEST_DIR", &f.store.data_dir)
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    timeout(Wait::from_secs(10), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line == "LOCKED" {
                return;
            }
        }
        panic!("child exited without acquiring its lock");
    })
    .await
    .unwrap();
    let w = window_at(DateTime::UNIX_EPOCH);
    let excluded = f.store.try_interpretation_guard(w).await.unwrap().is_none();
    let path = f
        .store
        .data_dir
        .join("locks/interpret-1970-01-01T00:00:00+00:00");
    let os_excluded = std::fs::File::open(&path)
        .map(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
        .unwrap_or(false);
    // A guard must leave the writer available to another connection.
    let writer = timeout(Wait::from_secs(1), f.store.begin_write()).await;
    let database_transaction = writer.is_err();
    if let Ok(tx) = writer {
        tx.unwrap().commit().await.unwrap();
    }
    child.kill().await.unwrap(); // SIGKILL on Unix; no destructors can release it.
    let status = child.wait().await.unwrap();
    let guard = f.store.try_interpretation_guard(w).await.unwrap();
    let reacquired = guard.is_some();
    let same_process_excluded = f.store.try_interpretation_guard(w).await.unwrap().is_none();
    drop(guard);
    let released_on_drop = f.store.try_interpretation_guard(w).await.unwrap().is_some();
    let stale_file = path.exists();
    f.close().await;
    assert!(!status.success());
    assert!(excluded, "a live child must exclude another provider run");
    assert!(os_excluded, "the guard must hold an OS file lock");
    assert!(
        !database_transaction,
        "a provider guard must not hold a transaction"
    );
    assert!(reacquired, "SIGKILL must release the OS lock");
    assert!(
        same_process_excluded,
        "separate opens in one process must conflict"
    );
    assert!(released_on_drop, "drop must release the lock immediately");
    assert!(
        stale_file,
        "the leftover lock file must not prevent reacquisition"
    );
}

/// Commit observation lives inside WriteTransaction::commit, before it returns.
/// Thus moving unlink after commit cannot race ahead of the test's observation.
/// The second deletion is ordered before upload admission, then paused after
/// commit until that upload finishes. Hooks exist only in unit-test builds.
#[derive(Default)]
pub(crate) struct GcBoundary {
    committed: tokio::sync::Notify,
    resume: tokio::sync::Notify,
    seen: std::sync::atomic::AtomicBool,
    deleting: tokio::sync::Notify,
    delete: tokio::sync::Notify,
    second_committed: tokio::sync::Notify,
    finish: tokio::sync::Notify,
}
impl GcBoundary {
    pub(crate) async fn before_unlink(&self) {
        if self.seen.load(std::sync::atomic::Ordering::SeqCst) {
            self.deleting.notify_one();
            self.delete.notified().await;
        }
    }
    pub(crate) async fn after_commit(&self) {
        if !self.seen.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.committed.notify_one();
            self.resume.notified().await;
        } else {
            self.second_committed.notify_one();
            self.finish.notified().await;
        }
    }
}

#[tokio::test]
async fn reupload_during_gc_keeps_its_file_and_does_not_wait_for_other_blobs() {
    let mut f = test_support::Fixture::new().await;
    let mut blobs = [
        b"synthetic chunk one".as_slice(),
        b"synthetic chunk two".as_slice(),
    ]
    .map(|bytes| (hash(bytes), bytes));
    blobs.sort_by(|a, b| a.0.cmp(&b.0));
    let [(first, bytes), (second, other_bytes)] = &blobs;
    f.store.upload_blob(first, bytes).await.unwrap();
    f.store.upload_blob(second, other_bytes).await.unwrap();
    f.store.age_blobs_for_retention("").await.unwrap();
    let boundary = std::sync::Arc::new(GcBoundary::default());
    f.store.gc_boundary = Some(boundary.clone());
    let gc_store = f.store.clone();
    let mut gc = tokio::spawn(async move { gc_store.gc().await });
    timeout(Wait::from_secs(10), boundary.committed.notified())
        .await
        .unwrap();
    let observer = Store::open(&f.store.data_dir).await.unwrap();
    let first_deleted: bool =
        sqlx::query_scalar("SELECT NOT published AND evidence_expired FROM blobs WHERE sha256=$1")
            .bind(first)
            .fetch_one(&observer.pool)
            .await
            .unwrap();
    assert!(
        first_deleted,
        "another connection must see the first committed deletion during the run"
    );
    assert!(!f.store.blob_path(first).unwrap().exists());
    assert!(
        !gc.is_finished(),
        "GC must still be part way through its run"
    );
    assert!(
        timeout(Wait::from_secs(2), observer.upload_blob(first, bytes))
            .await
            .expect("unrelated upload must complete before the whole GC run finishes")
            .unwrap()
    );
    assert!(!gc.is_finished());

    // Force GC to own the second blob's writer before starting its upload.
    boundary.resume.notify_one();
    timeout(Wait::from_secs(5), boundary.deleting.notified())
        .await
        .unwrap();
    let upload_store = observer.clone();
    let upload_sha = second.clone();
    let upload_bytes = other_bytes.to_vec();
    let mut reupload =
        tokio::spawn(async move { upload_store.upload_blob(&upload_sha, &upload_bytes).await });
    tokio::time::sleep(Wait::from_millis(50)).await;
    assert!(
        !reupload.is_finished(),
        "second writer must wait for the gate"
    );
    assert!(!gc.is_finished(), "GC must wait at the deletion barrier");
    boundary.delete.notify_one();
    timeout(Wait::from_secs(5), boundary.second_committed.notified())
        .await
        .unwrap();
    timeout(Wait::from_secs(5), &mut reupload)
        .await
        .expect("upload must succeed within busy_timeout")
        .unwrap()
        .unwrap();
    assert!(
        !gc.is_finished(),
        "upload must finish while GC is paused just after commit"
    );
    boundary.finish.notify_one();
    let expired = timeout(Wait::from_secs(5), &mut gc)
        .await
        .expect("GC must succeed within busy_timeout")
        .unwrap()
        .unwrap();
    assert!((1..=2).contains(&expired));
    for (sha, bytes) in &blobs {
        let published: bool = sqlx::query_scalar(
            "SELECT published AND NOT evidence_expired FROM blobs WHERE sha256=$1",
        )
        .bind(sha)
        .fetch_one(&observer.pool)
        .await
        .unwrap();
        assert!(published, "a racing upload must end published");
        assert_eq!(
            tokio::fs::read(observer.blob_path(sha).unwrap())
                .await
                .unwrap(),
            *bytes,
            "GC must never lose the re-uploaded file"
        );
    }
    observer.pool.close().await;
    f.close().await;
}

// A 200 tells the client it may discard its copy, so it has to mean the file is
// there and will stay for a full retention period.
#[tokio::test]
async fn reupload_restores_a_missing_file_and_restarts_retention() {
    let f = test_support::Fixture::new().await;
    let bytes = b"synthetic chunk";
    let sha = hash(bytes);
    let path = f.store.blob_path(&sha).unwrap();
    assert!(f.store.upload_blob(&sha, bytes).await.unwrap());

    // What a gc crash between its unlink and its commit leaves behind.
    std::fs::remove_file(&path).unwrap();
    f.store.upload_blob(&sha, bytes).await.unwrap();
    assert!(path.exists(), "upload acknowledged a blob with no file");

    sqlx::query("UPDATE blobs SET created_at=$2 WHERE sha256=$1")
        .bind(&sha)
        .bind(Ts::from(Utc::now() - Duration::days(31)))
        .execute(&f.store.pool)
        .await
        .unwrap();
    assert!(!f.store.upload_blob(&sha, bytes).await.unwrap());
    assert_eq!(f.store.gc().await.unwrap(), 0);
    assert!(path.exists(), "gc deleted a blob that was just re-uploaded");
}
