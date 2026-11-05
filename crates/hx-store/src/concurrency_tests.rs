use super::*;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Duration as Wait, timeout};

// Re-execute just this helper so SIGKILL tests the actual Store guard in a
// different process. The parent owns the synthetic database and its cleanup.
#[tokio::test]
#[ignore = "subprocess helper for interpretation_lock_releases_on_process_death_and_drop"]
async fn interpretation_lock_child() {
    let url = std::env::var("HX_LOCK_TEST_URL").unwrap();
    let dir = std::env::var("HX_LOCK_TEST_DIR").unwrap();
    let store = Store::connect(&url, dir).await.unwrap();
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
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    let test_url = std::env::var("HX_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://hx:hx@localhost:55432/hx".into());
    let url = format!("{}/{db}", test_url.rsplit_once('/').unwrap().0);
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "concurrency_tests::interpretation_lock_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HX_LOCK_TEST_URL", url)
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
    let database_transaction: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND state='idle in transaction')",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
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

#[tokio::test]
async fn reupload_during_gc_keeps_its_file_and_does_not_wait_for_other_blobs() {
    let f = test_support::Fixture::new().await;
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
    // Pause GC's second deletion at its UPDATE. The first deletion must already
    // be committed, allowing its re-upload while this unrelated blob is held.
    sqlx::raw_sql(
        "CREATE TABLE gc_barrier(id integer PRIMARY KEY); INSERT INTO gc_barrier VALUES(1);
        CREATE FUNCTION pause_gc() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
            IF OLD.sha256 = TG_ARGV[0] AND NOT NEW.published THEN
                PERFORM 1 FROM gc_barrier WHERE id=1 FOR UPDATE;
            END IF;
            RETURN NEW;
        END $$;",
    )
    .execute(&f.store.pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER pause_gc BEFORE UPDATE ON blobs FOR EACH ROW EXECUTE FUNCTION pause_gc('{second}')"
    )))
    .execute(&f.store.pool).await.unwrap();
    let mut barrier = f.store.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM gc_barrier FOR UPDATE")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let gc_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            f.store
                .pool
                .connect_options()
                .as_ref()
                .clone()
                .application_name("gc-race-test"),
        )
        .await
        .unwrap();
    let gc_store = Store::new(gc_pool.clone(), &f.store.data_dir);
    let gc = tokio::spawn(async move { gc_store.gc().await });
    timeout(Wait::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='gc-race-test' AND wait_event_type='Lock')")
                .fetch_one(&f.store.pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let upload_store = f.store.clone();
    let upload_sha = first.clone();
    let upload_bytes = bytes.to_vec();
    let mut upload =
        tokio::spawn(async move { upload_store.upload_blob(&upload_sha, &upload_bytes).await });
    let progressed = match timeout(Wait::from_secs(2), &mut upload).await {
        Ok(result) => {
            assert!(result.unwrap().unwrap());
            true
        }
        Err(_) => false,
    };
    // Also re-upload the blob whose deletion is still in flight. Upload must
    // publish after GC commits, rather than return early or lose its new file.
    let upload_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            f.store
                .pool
                .connect_options()
                .as_ref()
                .clone()
                .application_name("gc-upload-test"),
        )
        .await
        .unwrap();
    let upload_store = Store::new(upload_pool.clone(), &f.store.data_dir);
    let upload_sha = second.clone();
    let upload_bytes = other_bytes.to_vec();
    let reupload =
        tokio::spawn(async move { upload_store.upload_blob(&upload_sha, &upload_bytes).await });
    timeout(Wait::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='gc-upload-test' AND wait_event_type='Lock')")
                .fetch_one(&f.store.pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    barrier.commit().await.unwrap();
    assert_eq!(gc.await.unwrap().unwrap(), 2);
    assert!(reupload.await.unwrap().unwrap());
    if !progressed {
        upload.await.unwrap().unwrap();
    }
    let published: bool =
        sqlx::query_scalar("SELECT published AND NOT evidence_expired FROM blobs WHERE sha256=$1")
            .bind(first)
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
    let on_disk = tokio::fs::read(f.store.blob_path(first).unwrap())
        .await
        .unwrap();
    let other_on_disk = tokio::fs::read(f.store.blob_path(second).unwrap())
        .await
        .unwrap();
    gc_pool.close().await;
    upload_pool.close().await;
    f.close().await;
    assert!(
        progressed,
        "re-upload must not wait for GC of a different blob"
    );
    assert!(published);
    assert_eq!(on_disk, *bytes, "GC must not unlink the re-uploaded file");
    assert_eq!(
        other_on_disk, *other_bytes,
        "same-blob GC and re-upload must serialize"
    );
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

    sqlx::query("UPDATE blobs SET created_at=now()-interval '31 days' WHERE sha256=$1")
        .bind(&sha)
        .execute(&f.store.pool)
        .await
        .unwrap();
    assert!(!f.store.upload_blob(&sha, bytes).await.unwrap());
    assert_eq!(f.store.gc().await.unwrap(), 0);
    assert!(path.exists(), "gc deleted a blob that was just re-uploaded");
}
