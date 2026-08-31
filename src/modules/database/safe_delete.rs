// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.


use std::sync::{Arc, OnceLock};
use std::time::Duration;

use db_type::{KeyDefinition, KeyOptions, ToKeyDefinition};
use native_db::*;
use tokio::task::spawn_blocking;
use tracing::{error, info};

use crate::modules::error::{code::ErrorCode, RustMailerResult};
use crate::raise_error;

/// Filter used to select which scanned rows should be deleted.
pub type RowFilter<T> = Arc<dyn Fn(&T) -> bool + Send + Sync + 'static>;

/// Phase 1 (read-only): collect up to `limit` entities whose secondary key starts with
/// `start_with` and satisfy `filter`.
///
/// Runs in a read transaction on a blocking thread. If the scan hangs, no write lock is
/// held, so other DB writers keep working.
fn collect_secondary_range_sync<T, K>(
    database: &Arc<Database<'static>>,
    key_def: &KeyDefinition<KeyOptions>,
    start_with: K,
    filter: &RowFilter<T>,
    limit: usize,
) -> RustMailerResult<Vec<T>>
where
    T: ToInput + Clone,
    K: ToKey,
{
    let r_transaction = database
        .r_transaction()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let entities: Vec<T> = r_transaction
        .scan()
        .secondary(key_def.clone())
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .start_with(start_with)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .filter_map(Result::ok)
        .filter(|item: &T| filter(item))
        .take(limit)
        .collect();
    Ok(entities)
}

/// Phase 1 (read-only): collect up to `limit` entities whose secondary key starts with
/// `start_with` and satisfy `filter`.
///
/// Runs in a read transaction on a blocking thread. If the scan hangs, no write lock is
/// held, so other DB writers keep working.
pub async fn collect_secondary_range_impl<T>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + 'static,
    filter: RowFilter<T>,
    limit: usize,
) -> RustMailerResult<Vec<T>>
where
    T: ToInput + Clone + Send + 'static,
{
    let db = database.clone();
    spawn_blocking(move || {
        let key_def = key_def.key_definition();
        collect_secondary_range_sync(&db, &key_def, start_with, &filter, limit)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

/// Phase 2 (write-only): delete `to_delete` by primary key (synchronous core).
fn delete_by_primary_sync<T>(
    database: &Arc<Database<'static>>,
    to_delete: Vec<T>,
) -> RustMailerResult<usize>
where
    T: ToInput + Clone,
{
    if to_delete.is_empty() {
        return Ok(0);
    }
    let rw_transaction = database
        .rw_transaction()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let mut removed = 0usize;
    for item in to_delete {
        match rw_transaction.remove::<T>(item) {
            Ok(_) => removed += 1,
            // Deleted by another writer between the read and write phases: already done.
            Err(native_db::db_type::Error::KeyNotFound { .. }) => {}
            // Changed by another writer between the read and write phases: leave it
            // alone, the next collection pass re-evaluates the filter on fresh data.
            Err(native_db::db_type::Error::IncorrectInputData { .. }) => {}
            Err(e) => {
                return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
            }
        }
    }
    rw_transaction
        .commit()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(removed)
}

/// Phase 2 (write-only): delete `to_delete` by primary key.
///
/// The write transaction contains only bounded, keyed removals — no scans, no iterators,
/// so the write lock is held for the shortest possible time.
pub async fn delete_by_primary_impl<T>(
    database: &Arc<Database<'static>>,
    to_delete: Vec<T>,
) -> RustMailerResult<usize>
where
    T: ToInput + Clone + Send + 'static,
{
    if to_delete.is_empty() {
        return Ok(0);
    }
    let db = database.clone();
    spawn_blocking(move || delete_by_primary_sync(&db, to_delete))
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

/// Two-phase delete that merges the write side into a single transaction: collect ALL
/// matching rows in one read-only scan, then remove them all in one write transaction.
///
/// Use this when the expected number of matches is bounded (e.g. a known UID list).
/// For unbounded deletions (whole mailbox / account) prefer
/// [`batch_delete_secondary_impl`], which chunks the work so each write transaction
/// stays short.
pub async fn delete_secondary_impl<T>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + 'static,
    filter: RowFilter<T>,
) -> RustMailerResult<usize>
where
    T: ToInput + Clone + Send + 'static,
{
    let db = database.clone();
    let to_delete = spawn_blocking(move || {
        let key_def = key_def.key_definition();
        collect_secondary_range_sync(&db, &key_def, start_with, &filter, usize::MAX)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))??;
    delete_by_primary_impl(database, to_delete).await
}

/// Two-phase batch delete: read-collect then write-delete, chunked by `batch_size`.
///
/// This is a drop-in replacement for the current "scan + delete inside one write
/// transaction" pattern used by the stale-envelope cleanup. The `filter` is applied
/// inside the read scan (before `take`), preserving the current batching semantics.
pub async fn batch_delete_secondary_impl<T>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + Clone + 'static,
    filter: RowFilter<T>,
    batch_size: usize,
) -> RustMailerResult<usize>
where
    T: ToInput + Clone + Send + 'static,
{
    // Convert to the concrete, cloneable KeyDefinition once, so the loop can reuse it.
    let key_def = key_def.key_definition();
    let mut total_deleted = 0usize;
    loop {
        let batch = collect_secondary_range_impl::<T>(
            database,
            key_def.clone(),
            start_with.clone(),
            filter.clone(),
            batch_size,
        )
        .await?;
        if batch.is_empty() {
            break;
        }
        total_deleted += delete_by_primary_impl(database, batch).await?;
    }
    Ok(total_deleted)
}

/// Pause inserted between deletion chunks on the background worker, giving other
/// writers a window to acquire the (single) redb write lock.
const INTER_CHUNK_DELAY: Duration = Duration::from_millis(1);

/// Chunked two-phase delete with a short pause between chunks. Intended for the
/// background worker: the write lock is held only for one bounded chunk at a time,
/// and the pause lets other writers interleave.
fn batch_delete_secondary_paced_sync<T, K>(
    database: &Arc<Database<'static>>,
    key_def: &KeyDefinition<KeyOptions>,
    start_with: K,
    filter: &RowFilter<T>,
    batch_size: usize,
) -> RustMailerResult<usize>
where
    T: ToInput + Clone,
    K: ToKey + Clone,
{
    let mut total_deleted = 0usize;
    loop {
        let batch =
            collect_secondary_range_sync(database, key_def, start_with.clone(), filter, batch_size)?;
        if batch.is_empty() {
            break;
        }
        total_deleted += delete_by_primary_sync(database, batch)?;
        std::thread::sleep(INTER_CHUNK_DELAY);
    }
    Ok(total_deleted)
}

/// A queued deletion job: `run` performs the whole chunked delete, `label` is used
/// for worker-side logging.
struct DeleteJob {
    label: String,
    run: Box<dyn FnOnce() -> RustMailerResult<usize> + Send + 'static>,
}

struct DeleteWorker {
    tx: std::sync::mpsc::Sender<DeleteJob>,
}

static DELETE_WORKER: OnceLock<DeleteWorker> = OnceLock::new();

fn delete_worker() -> &'static DeleteWorker {
    DELETE_WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<DeleteJob>();
        std::thread::Builder::new()
            .name("db-delete-worker".to_string())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    // Keep the worker alive even if a job panics, otherwise every
                    // queued deletion behind it would be stuck forever.
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.run)()));
                    match result {
                        Ok(Ok(deleted)) => info!(
                            "[safe_delete] {} finished, deleted {} rows",
                            job.label, deleted
                        ),
                        Ok(Err(e)) => error!("[safe_delete] {} failed: {:#?}", job.label, e),
                        Err(_) => error!("[safe_delete] {} panicked", job.label),
                    }
                }
            })
            .expect("failed to spawn delete worker thread");
        DeleteWorker { tx }
    })
}

/// Enqueue a two-phase delete to be processed in the background.
///
/// The caller returns immediately. A dedicated worker thread drains the queue one job
/// at a time, deleting in small chunks with a pause between chunks, so the single
/// write lock is never held for long and other tasks keep making progress.
pub fn enqueue_delete_secondary_impl<T, K>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: K,
    filter: RowFilter<T>,
    batch_size: usize,
    label: impl Into<String>,
) -> RustMailerResult<()>
where
    T: ToInput + Clone + Send + 'static,
    K: ToKey + Send + Clone + 'static,
{
    let db = database.clone();
    let label = label.into();
    let job = DeleteJob {
        label,
        run: Box::new(move || {
            let key_def = key_def.key_definition();
            batch_delete_secondary_paced_sync(&db, &key_def, start_with, &filter, batch_size)
        }),
    };
    delete_worker()
        .tx
        .send(job)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::database::batch_delete_impl;
    use itertools::Itertools;
    use native_model::{native_model, Model};
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use std::sync::LazyLock;
    use std::time::{Duration, Instant};

    const BATCH_SIZE: usize = 200;

    /// Test model mirroring the shape of `MinimalEnvelope` / `EmailEnvelopeV3`
    /// (String-ish primary key + `account_id` / `mailbox_id` secondary keys).
    #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
    #[native_model(id = 9001, version = 1)]
    #[native_db(primary_key(pk -> String))]
    struct TestEnvelope {
        #[secondary_key]
        account_id: u64,
        #[secondary_key]
        mailbox_id: u64,
        uid: u32,
        data: String,
    }

    impl TestEnvelope {
        fn pk(&self) -> String {
            format!("{}_{}_{}", self.account_id, self.mailbox_id, self.uid)
        }
    }

    static TEST_MODELS: LazyLock<Models> = LazyLock::new(|| {
        let mut models = Models::new();
        models.define::<TestEnvelope>().unwrap();
        models
    });

    fn new_db() -> Arc<Database<'static>> {
        Arc::new(Builder::new().create_in_memory(&TEST_MODELS).unwrap())
    }

    fn seed(db: &Arc<Database<'static>>, accounts: u64, mailboxes: u64, per_mailbox: u32) {
        let mut rows = Vec::new();
        for account in 1..=accounts {
            for mailbox in 1..=mailboxes {
                for uid in 1..=per_mailbox {
                    rows.push(TestEnvelope {
                        account_id: account,
                        mailbox_id: mailbox,
                        uid,
                        data: format!("{account}-{mailbox}-{uid}"),
                    });
                }
            }
        }
        let rw = db.rw_transaction().unwrap();
        for row in rows {
            rw.insert(row).unwrap();
        }
        rw.commit().unwrap();
    }

    fn snapshot(db: &Arc<Database<'static>>) -> Vec<TestEnvelope> {
        let r = db.r_transaction().unwrap();
        let mut rows: Vec<TestEnvelope> =
            r.scan().primary().unwrap().all().unwrap().try_collect().unwrap();
        rows.sort_by(|a, b| a.pk().cmp(&b.pk()));
        rows
    }

    fn count_rows(db: &Arc<Database<'static>>) -> usize {
        snapshot(db).len()
    }

    /// Exact replica of the current production pattern (`batch_delete_impl` + the
    /// `EmailEnvelopeV3::clean_envelopes` closure): scan + delete inside ONE write tx.
    async fn delete_current_pattern(
        db: &Arc<Database<'static>>,
        account_id: u64,
        mailbox_id: u64,
        uids: &[u32],
        slow_scan: Option<Duration>,
    ) -> usize {
        let to_delete_set: HashSet<u32> = uids.iter().copied().collect();
        let to_delete_set = Arc::new(to_delete_set);
        let mut total_deleted = 0usize;
        loop {
            let db = db.clone();
            let set = to_delete_set.clone();
            let deleted = spawn_blocking(move || {
                let rw = db.rw_transaction().map_err(|e| format!("{e:?}"))?;
                let to_delete: Vec<TestEnvelope> = rw
                    .scan()
                    .secondary(TestEnvelopeKey::mailbox_id)
                    .map_err(|e| format!("{e:?}"))?
                    .start_with(mailbox_id)
                    .map_err(|e| format!("{e:?}"))?
                    .filter_map(Result::ok)
                    .filter(|e: &TestEnvelope| {
                        if let Some(sleep) = slow_scan {
                            std::thread::sleep(sleep);
                        }
                        e.account_id == account_id && set.contains(&e.uid)
                    })
                    .take(BATCH_SIZE)
                    .collect();
                let count = to_delete.len();
                for item in to_delete {
                    rw.remove::<TestEnvelope>(item)
                        .map_err(|e| format!("{e:?}"))?;
                }
                rw.commit().map_err(|e| format!("{e:?}"))?;
                Ok::<usize, String>(count)
            })
            .await
            .expect("blocking task panicked")
            .expect("delete failed");
            total_deleted += deleted;
            if deleted == 0 {
                break;
            }
        }
        total_deleted
    }

    /// Fixed pattern, expressed through the new module helpers.
    async fn delete_safe_pattern(
        db: &Arc<Database<'static>>,
        account_id: u64,
        mailbox_id: u64,
        uids: &[u32],
    ) -> usize {
        let to_delete_set: HashSet<u32> = uids.iter().copied().collect();
        let to_delete_set = Arc::new(to_delete_set);
        let filter: RowFilter<TestEnvelope> = Arc::new(move |e: &TestEnvelope| {
            e.account_id == account_id && to_delete_set.contains(&e.uid)
        });
        batch_delete_secondary_impl(
            db,
            TestEnvelopeKey::mailbox_id,
            mailbox_id,
            filter,
            BATCH_SIZE,
        )
        .await
        .unwrap()
    }

    /// Merged single-write-transaction pattern through the new module helpers.
    async fn delete_merged_pattern(
        db: &Arc<Database<'static>>,
        account_id: u64,
        mailbox_id: u64,
        uids: &[u32],
    ) -> usize {
        let to_delete_set: HashSet<u32> = uids.iter().copied().collect();
        let to_delete_set = Arc::new(to_delete_set);
        let filter: RowFilter<TestEnvelope> = Arc::new(move |e: &TestEnvelope| {
            e.account_id == account_id && to_delete_set.contains(&e.uid)
        });
        delete_secondary_impl(db, TestEnvelopeKey::mailbox_id, mailbox_id, filter)
            .await
            .unwrap()
    }

    /// All three approaches must produce the same end state for the same deletion.
    #[tokio::test]
    async fn current_vs_safe_delete_equivalent() {
        let uids: Vec<u32> = (1..=450).collect(); // spans multiple batches

        let db_current = new_db();
        seed(&db_current, 3, 2, 500);
        let deleted_current = delete_current_pattern(&db_current, 2, 1, &uids, None).await;
        assert_eq!(deleted_current, uids.len());

        let db_safe = new_db();
        seed(&db_safe, 3, 2, 500);
        let deleted_safe = delete_safe_pattern(&db_safe, 2, 1, &uids).await;
        assert_eq!(deleted_safe, uids.len());

        let db_merged = new_db();
        seed(&db_merged, 3, 2, 500);
        let deleted_merged = delete_merged_pattern(&db_merged, 2, 1, &uids).await;
        assert_eq!(deleted_merged, uids.len());

        // Same remaining rows, same counts, other mailboxes/accounts untouched.
        assert_eq!(count_rows(&db_current), count_rows(&db_safe));
        assert_eq!(snapshot(&db_current), snapshot(&db_safe));
        assert_eq!(count_rows(&db_current), count_rows(&db_merged));
        assert_eq!(snapshot(&db_current), snapshot(&db_merged));
    }

    /// Rows deleted or modified by another writer between the read and write phases
    /// must be skipped instead of failing the whole batch.
    #[tokio::test]
    async fn delete_by_primary_skips_missing_and_changed_rows() {
        let db = new_db();
        let rows = vec![
            TestEnvelope {
                account_id: 1,
                mailbox_id: 1,
                uid: 1,
                data: "a".into(),
            },
            TestEnvelope {
                account_id: 1,
                mailbox_id: 1,
                uid: 2,
                data: "b".into(),
            },
            TestEnvelope {
                account_id: 1,
                mailbox_id: 1,
                uid: 3,
                data: "c".into(),
            },
        ];
        let rw = db.rw_transaction().unwrap();
        for row in rows.iter().cloned() {
            rw.insert(row).unwrap();
        }
        rw.commit().unwrap();

        // Row 1 is deleted by another writer before our delete phase runs.
        let rw = db.rw_transaction().unwrap();
        rw.remove::<TestEnvelope>(rows[0].clone()).unwrap();
        rw.commit().unwrap();

        // Row 2 is updated (same primary key, different value) before our delete runs.
        let rw = db.rw_transaction().unwrap();
        let mut changed = rows[1].clone();
        changed.data = "b-updated".into();
        rw.update(rows[1].clone(), changed.clone()).unwrap();
        rw.commit().unwrap();

        // Row 3 is untouched. Deleting all three stale copies must skip 1 & 2 and
        // only count row 3 as removed.
        let deleted = delete_by_primary_impl(&db, rows).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining = snapshot(&db);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0], changed);
    }

    /// Background deletion: enqueue returns immediately, the worker drains the queue,
    /// and the end state matches a synchronous deletion.
    #[tokio::test]
    async fn enqueued_delete_processes_in_background() {
        let db = new_db();
        seed(&db, 2, 2, 100); // 400 rows: account 1 = 200, account 2 = 200

        let to_delete_set: HashSet<u32> = (1..=250).collect();
        let to_delete_set = Arc::new(to_delete_set);
        let filter: RowFilter<TestEnvelope> = Arc::new(move |e: &TestEnvelope| {
            e.account_id == 1 && to_delete_set.contains(&e.uid)
        });
        // start_with(mailbox_id) only covers one mailbox, so enqueue one job per mailbox.
        for mailbox_id in [1u64, 2u64] {
            enqueue_delete_secondary_impl(
                &db,
                TestEnvelopeKey::mailbox_id,
                mailbox_id,
                filter.clone(),
                BATCH_SIZE,
                "test-enqueued-delete",
            )
            .unwrap();
        }

        // The worker runs in the background; poll until it catches up.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = count_rows(&db);
            if remaining == 200 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "queued delete did not finish in time, remaining={remaining}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Everything left belongs to account 2.
        for row in snapshot(&db) {
            assert_eq!(row.account_id, 2);
        }
    }

    /// Production point-lookup delete (the stale-envelope cleanup fix): one bounded
    /// write tx, one lookup per UID, no full mailbox scan. Each entity type must
    /// remove exactly the target rows and leave everything else untouched.
    #[tokio::test]
    async fn point_lookup_delete_only_removes_target_rows() {
        let db = new_bench_db();
        seed_minimal(&db);
        seed_address(&db);
        seed_thread(&db);
        seed_envelope_v3(&db);

        let uids: Vec<u32> = vec![3, 17, 99];
        let addr_hash = |uid: u32| (uid as u64).wrapping_mul(0x9E3779B97F4A7C15);

        // One envelope can produce several AddressEntity rows (to/cc/...): all rows
        // sharing the envelope_hash must go, not just one.
        let extra = [
            BenchAddress {
                id: 9_000_001,
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                envelope_hash: addr_hash(3),
                note: "extra-1".into(),
            },
            BenchAddress {
                id: 9_000_002,
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                envelope_hash: addr_hash(3),
                note: "extra-2".into(),
            },
        ];
        let rw = db.rw_transaction().unwrap();
        for row in extra.iter().cloned() {
            rw.insert(row).unwrap();
        }
        rw.commit().unwrap();

        // MinimalEnvelope: u64 primary key IS envelope_hash.
        let hashes: Vec<u64> = uids
            .iter()
            .map(|uid| BenchMinimal {
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                uid: *uid,
                flags_hash: 0,
            }
            .pk())
            .collect();
        let deleted = batch_delete_impl(&db, move |rw| {
            let mut to_delete = Vec::with_capacity(hashes.len());
            for hash in hashes {
                if let Some(e) = rw
                    .get()
                    .primary::<BenchMinimal>(hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                {
                    to_delete.push(e);
                }
            }
            Ok(to_delete)
        })
        .await
        .unwrap();
        assert_eq!(deleted, uids.len());

        // AddressEntity: non-unique envelope_hash point scan.
        let hashes: Vec<u64> = uids.iter().map(|uid| addr_hash(*uid)).collect();
        let deleted = batch_delete_impl(&db, move |rw| {
            let mut to_delete = Vec::new();
            for hash in hashes {
                let rows: Vec<BenchAddress> = rw
                    .scan()
                    .secondary(BenchAddressKey::envelope_hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .start_with(hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .filter_map(Result::ok)
                    .collect();
                to_delete.extend(rows);
            }
            Ok(to_delete)
        })
        .await
        .unwrap();
        // 3 envelopes + 2 extra AddressEntity rows that share uid-3's hash.
        assert_eq!(deleted, uids.len() + 2);

        // EmailThread: unique envelope_id secondary lookup.
        let hashes: Vec<u64> = uids.iter().map(|uid| *uid as u64).collect();
        let deleted = batch_delete_impl(&db, move |rw| {
            let mut to_delete = Vec::with_capacity(hashes.len());
            for hash in hashes {
                if let Some(t) = rw
                    .get()
                    .secondary::<BenchThread>(BenchThreadKey::envelope_id, hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                {
                    to_delete.push(t);
                }
            }
            Ok(to_delete)
        })
        .await
        .unwrap();
        assert_eq!(deleted, uids.len());

        // EmailEnvelopeV3: unique create_envelope_id secondary lookup.
        let hashes: Vec<u64> = uids.iter().map(|uid| addr_hash(*uid)).collect();
        let deleted = batch_delete_impl(&db, move |rw| {
            let mut to_delete = Vec::with_capacity(hashes.len());
            for hash in hashes {
                if let Some(e) = rw
                    .get()
                    .secondary::<BenchEnvelopeV3>(BenchEnvelopeV3Key::create_envelope_id, hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                {
                    to_delete.push(e);
                }
            }
            Ok(to_delete)
        })
        .await
        .unwrap();
        assert_eq!(deleted, uids.len());

        // Only the target rows were removed; every other row survives.
        let r = db.r_transaction().unwrap();
        let expect = BN - uids.len() as u64;
        assert_eq!(r.len().primary::<BenchMinimal>().unwrap(), expect);
        assert_eq!(r.len().primary::<BenchAddress>().unwrap(), expect);
        assert_eq!(r.len().primary::<BenchThread>().unwrap(), expect);
        assert_eq!(r.len().primary::<BenchEnvelopeV3>().unwrap(), expect);

        let pk = |uid: u32| {
            BenchMinimal {
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                uid,
                flags_hash: 0,
            }
            .pk()
        };
        assert!(r.get().primary::<BenchMinimal>(pk(1)).unwrap().is_some());
        assert!(r.get().primary::<BenchMinimal>(pk(3)).unwrap().is_none());
        assert!(r
            .get()
            .secondary::<BenchEnvelopeV3>(BenchEnvelopeV3Key::create_envelope_id, addr_hash(1))
            .unwrap()
            .is_some());
        assert!(r
            .get()
            .secondary::<BenchEnvelopeV3>(BenchEnvelopeV3Key::create_envelope_id, addr_hash(17))
            .unwrap()
            .is_none());
    }

    /// Failure mode of the current design: a scan that stalls inside the write tx
    /// blocks every other writer in the process.
    #[test]
    fn current_pattern_hung_scan_blocks_all_writers() {
        let db = new_db();
        seed(&db, 1, 1, 100);

        std::thread::scope(|scope| {
            let db_writer = Arc::clone(&db);
            scope.spawn(move || {
                // Simulate the production hang: the scan inside the write tx never
                // returns, so the write transaction is held indefinitely.
                let rw = db_writer.rw_transaction().unwrap();
                std::thread::sleep(Duration::from_millis(1200));
                rw.commit().unwrap();
            });

            // Give the stuck writer time to grab the lock first.
            std::thread::sleep(Duration::from_millis(200));

            let t0 = Instant::now();
            let rw = db.rw_transaction().unwrap();
            let blocked = t0.elapsed();
            rw.abort().unwrap();

            println!("[current] second writer was blocked for {blocked:?}");
            assert!(
                blocked >= Duration::from_millis(800),
                "second writer should have been blocked by the in-flight write tx, blocked={blocked:?}"
            );
        });
    }

    /// Containment of the fixed design: a scan that stalls in the *read* phase does
    /// NOT block writers.
    #[test]
    fn safe_pattern_hung_scan_does_not_block_writers() {
        let db = new_db();
        seed(&db, 1, 1, 100);

        std::thread::scope(|scope| {
            let db_reader = Arc::clone(&db);
            scope.spawn(move || {
                // Simulate a stuck read-phase scan (collect_secondary_range_impl).
                let r = db_reader.r_transaction().unwrap();
                std::thread::sleep(Duration::from_millis(1200));
                let rows: Vec<TestEnvelope> =
                    r.scan().primary().unwrap().all().unwrap().try_collect().unwrap();
                let _count = rows.len();
            });

            // Give the stuck reader time to open its read tx.
            std::thread::sleep(Duration::from_millis(200));

            let t0 = Instant::now();
            let rw = db.rw_transaction().unwrap();
            rw.insert(TestEnvelope {
                account_id: 9,
                mailbox_id: 9,
                uid: 1,
                data: "writer-won".into(),
            })
            .unwrap();
            rw.commit().unwrap();
            let elapsed = t0.elapsed();

            println!("[safe] writer completed while read tx was held in {elapsed:?}");
            assert!(
                elapsed < Duration::from_millis(800),
                "writer should not wait for a read transaction, elapsed={elapsed:?}"
            );
        });
    }

    // ---------------------------------------------------------------------------
    // Benchmark: 4-entity deletion throughput (10k rows each, half deleted)
    // ---------------------------------------------------------------------------

    /// Mirror of `MinimalEnvelope` — u64 primary key, account_id + mailbox_id secondary.
    #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
    #[native_model(id = 9002, version = 1)]
    #[native_db(primary_key(pk -> u64))]
    struct BenchMinimal {
        #[secondary_key]
        account_id: u64,
        #[secondary_key]
        mailbox_id: u64,
        uid: u32,
        flags_hash: u64,
    }
    impl BenchMinimal {
        fn pk(&self) -> u64 {
            self.account_id
                .wrapping_mul(0x100000001u64)
                .wrapping_add(self.mailbox_id)
                .wrapping_mul(0x100000001u64)
                .wrapping_add(self.uid as u64)
        }
    }

    /// Mirror of `AddressEntity` — u64 primary key, account_id + mailbox_id + envelope_hash secondary.
    #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
    #[native_model(id = 9003, version = 1)]
    #[native_db(primary_key(pk -> u64))]
    struct BenchAddress {
        id: u64,
        #[secondary_key]
        account_id: u64,
        #[secondary_key]
        mailbox_id: u64,
        #[secondary_key]
        envelope_hash: u64,
        note: String,
    }
    impl BenchAddress {
        fn pk(&self) -> u64 { self.id }
    }

    /// Mirror of `EmailThread` — String primary key, thread_id + envelope_id (unique) + account_id + mailbox_id secondary.
    #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
    #[native_model(id = 9004, version = 1)]
    #[native_db(
        primary_key(pk -> String),
        secondary_key(thread_id -> u64, unique),
        secondary_key(envelope_id -> u64, unique)
    )]
    struct BenchThread {
        #[secondary_key]
        account_id: u64,
        #[secondary_key]
        mailbox_id: u64,
        thread_id: u64,
        envelope_id: u64,
    }
    impl BenchThread {
        fn pk(&self) -> String {
            format!("{}_{}", self.thread_id, self.envelope_id)
        }
        fn thread_id(&self) -> u64 { self.thread_id }
        fn envelope_id(&self) -> u64 { self.envelope_id }
    }

    /// Mirror of `EmailEnvelopeV3` — String primary key, create_envelope_id (unique) + account_id + mailbox_id secondary.
    #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
    #[native_model(id = 9005, version = 1)]
    #[native_db(primary_key(pk -> String), secondary_key(create_envelope_id -> u64, unique))]
    struct BenchEnvelopeV3 {
        #[secondary_key]
        account_id: u64,
        #[secondary_key]
        mailbox_id: u64,
        uid: u32,
        create_envelope_id: u64,
        subject: String,
    }
    impl BenchEnvelopeV3 {
        fn pk(&self) -> String {
            format!("{}_{}", self.create_envelope_id, self.uid)
        }
        fn create_envelope_id(&self) -> u64 { self.create_envelope_id }
    }

    static BENCH_MODELS: LazyLock<Models> = LazyLock::new(|| {
        let mut m = Models::new();
        m.define::<BenchMinimal>().unwrap();
        m.define::<BenchAddress>().unwrap();
        m.define::<BenchThread>().unwrap();
        m.define::<BenchEnvelopeV3>().unwrap();
        m
    });

    fn new_bench_db() -> Arc<Database<'static>> {
        Arc::new(Builder::new().create_in_memory(&BENCH_MODELS).unwrap())
    }

    // 10k rows per entity; delete UIDs 5001..=10000 (half)
    const BN: u64 = 10_000;
    const BN_ACCT: u64 = 1;
    const BN_MB: u64 = 1;

    // ── MinimalEnvelope helpers ──────────────────────────────────────────────

    fn seed_minimal(db: &Arc<Database<'static>>) {
        let rows: Vec<BenchMinimal> = (1u32..=BN as u32)
            .map(|uid| BenchMinimal {
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                uid,
                flags_hash: uid as u64,
            })
            .collect();
        let rw = db.rw_transaction().unwrap();
        for r in rows {
            rw.insert(r).unwrap();
        }
        rw.commit().unwrap();
    }

    async fn bench_current_minimal(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u32>>,
    ) -> (usize, Duration) {
        let mut total = 0usize;
        let t0 = Instant::now();
        loop {
            let db = db.clone();
            let set = to_delete_set.clone();
            let n = spawn_blocking(move || {
                let rw = db.rw_transaction().unwrap();
                let rows: Vec<BenchMinimal> = rw
                    .scan()
                    .secondary(BenchMinimalKey::mailbox_id)
                    .unwrap()
                    .start_with(BN_MB)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e: &BenchMinimal| {
                        e.account_id == BN_ACCT && set.contains(&e.uid)
                    })
                    .take(BATCH_SIZE)
                    .collect();
                let count = rows.len();
                for r in rows {
                    rw.remove::<BenchMinimal>(r).unwrap();
                }
                rw.commit().unwrap();
                count
            })
            .await.unwrap();
            total += n;
            if n == 0 {
                break;
            }
        }
        (total, t0.elapsed())
    }

    async fn bench_safe_minimal(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u32>>,
    ) -> (usize, Duration) {
        let filter: RowFilter<BenchMinimal> = Arc::new(move |e: &BenchMinimal| {
            e.account_id == BN_ACCT && to_delete_set.contains(&e.uid)
        });
        let t0 = Instant::now();
        let total = batch_delete_secondary_impl(
            db,
            BenchMinimalKey::mailbox_id,
            BN_MB,
            filter,
            BATCH_SIZE,
        )
        .await
        .unwrap();
        (total, t0.elapsed())
    }

    // ── AddressEntity helpers ────────────────────────────────────────────────

    fn seed_address(db: &Arc<Database<'static>>) {
        let rows: Vec<BenchAddress> = (1u32..=BN as u32)
            .map(|uid| BenchAddress {
                id: uid as u64,
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                envelope_hash: (uid as u64).wrapping_mul(0x9E3779B97F4A7C15),
                note: format!("addr-{uid}"),
            })
            .collect();
        let rw = db.rw_transaction().unwrap();
        for r in rows {
            rw.insert(r).unwrap();
        }
        rw.commit().unwrap();
    }

    async fn bench_current_address(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u64>>,
    ) -> (usize, Duration) {
        let mut total = 0usize;
        let t0 = Instant::now();
        loop {
            let db = db.clone();
            let set = to_delete_set.clone();
            let n = spawn_blocking(move || {
                let rw = db.rw_transaction().unwrap();
                let rows: Vec<BenchAddress> = rw
                    .scan()
                    .secondary(BenchAddressKey::mailbox_id)
                    .unwrap()
                    .start_with(BN_MB)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e: &BenchAddress| {
                        e.account_id == BN_ACCT && set.contains(&e.envelope_hash)
                    })
                    .take(BATCH_SIZE)
                    .collect();
                let count = rows.len();
                for r in rows {
                    rw.remove::<BenchAddress>(r).unwrap();
                }
                rw.commit().unwrap();
                count
            })
            .await.unwrap();
            total += n;
            if n == 0 {
                break;
            }
        }
        (total, t0.elapsed())
    }

    async fn bench_safe_address(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u64>>,
    ) -> (usize, Duration) {
        let filter: RowFilter<BenchAddress> = Arc::new(move |e: &BenchAddress| {
            e.account_id == BN_ACCT && to_delete_set.contains(&e.envelope_hash)
        });
        let t0 = Instant::now();
        let total = batch_delete_secondary_impl(
            db,
            BenchAddressKey::mailbox_id,
            BN_MB,
            filter,
            BATCH_SIZE,
        )
        .await
        .unwrap();
        (total, t0.elapsed())
    }

    // ── EmailThread helpers ──────────────────────────────────────────────────

    fn seed_thread(db: &Arc<Database<'static>>) {
        let rows: Vec<BenchThread> = (1u32..=BN as u32)
            .map(|uid| BenchThread {
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                thread_id: uid as u64,
                envelope_id: uid as u64,
            })
            .collect();
        let rw = db.rw_transaction().unwrap();
        for r in rows {
            rw.insert(r).unwrap();
        }
        rw.commit().unwrap();
    }

    async fn bench_current_thread(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u64>>,
    ) -> (usize, Duration) {
        let mut total = 0usize;
        let t0 = Instant::now();
        loop {
            let db = db.clone();
            let set = to_delete_set.clone();
            let n = spawn_blocking(move || {
                let rw = db.rw_transaction().unwrap();
                let rows: Vec<BenchThread> = rw
                    .scan()
                    .secondary(BenchThreadKey::mailbox_id)
                    .unwrap()
                    .start_with(BN_MB)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e: &BenchThread| {
                        e.account_id == BN_ACCT && set.contains(&e.envelope_id)
                    })
                    .take(BATCH_SIZE)
                    .collect();
                let count = rows.len();
                for r in rows {
                    rw.remove::<BenchThread>(r).unwrap();
                }
                rw.commit().unwrap();
                count
            })
            .await.unwrap();
            total += n;
            if n == 0 {
                break;
            }
        }
        (total, t0.elapsed())
    }

    async fn bench_safe_thread(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u64>>,
    ) -> (usize, Duration) {
        let filter: RowFilter<BenchThread> = Arc::new(move |e: &BenchThread| {
            e.account_id == BN_ACCT && to_delete_set.contains(&e.envelope_id)
        });
        let t0 = Instant::now();
        let total = batch_delete_secondary_impl(
            db,
            BenchThreadKey::mailbox_id,
            BN_MB,
            filter,
            BATCH_SIZE,
        )
        .await
        .unwrap();
        (total, t0.elapsed())
    }

    // ── EmailEnvelopeV3 helpers ──────────────────────────────────────────────

    fn seed_envelope_v3(db: &Arc<Database<'static>>) {
        let rows: Vec<BenchEnvelopeV3> = (1u32..=BN as u32)
            .map(|uid| BenchEnvelopeV3 {
                account_id: BN_ACCT,
                mailbox_id: BN_MB,
                uid,
                create_envelope_id: (uid as u64).wrapping_mul(0x9E3779B97F4A7C15),
                subject: format!("subj-{uid}"),
            })
            .collect();
        let rw = db.rw_transaction().unwrap();
        for r in rows {
            rw.insert(r).unwrap();
        }
        rw.commit().unwrap();
    }

    async fn bench_current_envelope_v3(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u32>>,
    ) -> (usize, Duration) {
        let mut total = 0usize;
        let t0 = Instant::now();
        loop {
            let db = db.clone();
            let set = to_delete_set.clone();
            let n = spawn_blocking(move || {
                let rw = db.rw_transaction().unwrap();
                let rows: Vec<BenchEnvelopeV3> = rw
                    .scan()
                    .secondary(BenchEnvelopeV3Key::mailbox_id)
                    .unwrap()
                    .start_with(BN_MB)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e: &BenchEnvelopeV3| {
                        e.account_id == BN_ACCT && set.contains(&e.uid)
                    })
                    .take(BATCH_SIZE)
                    .collect();
                let count = rows.len();
                for r in rows {
                    rw.remove::<BenchEnvelopeV3>(r).unwrap();
                }
                rw.commit().unwrap();
                count
            })
            .await.unwrap();
            total += n;
            if n == 0 {
                break;
            }
        }
        (total, t0.elapsed())
    }

    async fn bench_safe_envelope_v3(
        db: &Arc<Database<'static>>,
        to_delete_set: Arc<HashSet<u32>>,
    ) -> (usize, Duration) {
        let filter: RowFilter<BenchEnvelopeV3> = Arc::new(move |e: &BenchEnvelopeV3| {
            e.account_id == BN_ACCT && to_delete_set.contains(&e.uid)
        });
        let t0 = Instant::now();
        let total = batch_delete_secondary_impl(
            db,
            BenchEnvelopeV3Key::mailbox_id,
            BN_MB,
            filter,
            BATCH_SIZE,
        )
        .await
        .unwrap();
        (total, t0.elapsed())
    }

    /// Main benchmark: runs both patterns on all 4 entity types with 10k rows each.
    #[tokio::test]
    async fn benchmark_four_entity_delete_throughput() {
        let uids_minimal: Vec<u32> = (5001u32..=10000).collect();
        let set_minimal: Arc<HashSet<u32>> = Arc::new(uids_minimal.iter().copied().collect());
        let uids_addr: Vec<u64> = (5001u64..=10000)
            .map(|u| u.wrapping_mul(0x9E3779B97F4A7C15))
            .collect();
        let set_addr: Arc<HashSet<u64>> = Arc::new(uids_addr.iter().copied().collect());
        let set_thread: Arc<HashSet<u64>> = Arc::new((5001u64..=10000).collect());
        let set_env: Arc<HashSet<u32>> = Arc::new(uids_minimal.iter().copied().collect());

        // ── MinimalEnvelope ──────────────────────────────────────────────────
        let db_cur = new_bench_db();
        seed_minimal(&db_cur);
        let (n_cur, t_cur) = bench_current_minimal(&db_cur, set_minimal.clone()).await;

        let db_safe = new_bench_db();
        seed_minimal(&db_safe);
        let (n_safe, t_safe) = bench_safe_minimal(&db_safe, set_minimal).await;

        println!(
            "[MinimalEnvelope]  current={n_cur} rows in {t_cur:?}  safe={n_safe} rows in {t_safe:?}"
        );
        assert_eq!(n_cur, (BN / 2) as usize);
        assert_eq!(n_safe, (BN / 2) as usize);

        // ── AddressEntity ────────────────────────────────────────────────────
        let db_cur = new_bench_db();
        seed_address(&db_cur);
        let (n_cur, t_cur) = bench_current_address(&db_cur, set_addr.clone()).await;

        let db_safe = new_bench_db();
        seed_address(&db_safe);
        let (n_safe, t_safe) = bench_safe_address(&db_safe, set_addr).await;

        println!(
            "[AddressEntity]    current={n_cur} rows in {t_cur:?}  safe={n_safe} rows in {t_safe:?}"
        );
        assert_eq!(n_cur, (BN / 2) as usize);
        assert_eq!(n_safe, (BN / 2) as usize);

        // ── EmailThread ──────────────────────────────────────────────────────
        let db_cur = new_bench_db();
        seed_thread(&db_cur);
        let (n_cur, t_cur) = bench_current_thread(&db_cur, set_thread.clone()).await;

        let db_safe = new_bench_db();
        seed_thread(&db_safe);
        let (n_safe, t_safe) = bench_safe_thread(&db_safe, set_thread).await;

        println!(
            "[EmailThread]      current={n_cur} rows in {t_cur:?}  safe={n_safe} rows in {t_safe:?}"
        );
        assert_eq!(n_cur, (BN / 2) as usize);
        assert_eq!(n_safe, (BN / 2) as usize);

        // ── EmailEnvelopeV3 ──────────────────────────────────────────────────
        let db_cur = new_bench_db();
        seed_envelope_v3(&db_cur);
        let (n_cur, t_cur) = bench_current_envelope_v3(&db_cur, set_env.clone()).await;

        let db_safe = new_bench_db();
        seed_envelope_v3(&db_safe);
        let (n_safe, t_safe) = bench_safe_envelope_v3(&db_safe, set_env).await;

        println!(
            "[EmailEnvelopeV3]  current={n_cur} rows in {t_cur:?}  safe={n_safe} rows in {t_safe:?}"
        );
        assert_eq!(n_cur, (BN / 2) as usize);
        assert_eq!(n_safe, (BN / 2) as usize);
    }
}
