//! The migration between the SQLite Contacts and engine stores.

mod conformance;

use contacts_plugin::store::{ContactsStore, SqliteContactsStore};
use platform_objects::store::sqlite::SqliteObjectStore;
use platform_objects::ObjectStore;
use platform_storage::sqlite::SqliteTenantDatabases;
use std::sync::Arc;

#[tokio::test]
async fn a_sqlite_workspace_moves_to_the_engine() {
    let directory = tempfile::tempdir().expect("temp dir");
    let contacts = Arc::new(SqliteTenantDatabases::new(
        directory.path().join("contacts"),
        contacts_plugin::store::sqlite::SQLITE_MIGRATIONS,
    ));
    let engine = Arc::new(SqliteTenantDatabases::new(
        directory.path().join("engine"),
        platform_objects::store::sqlite::SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ContactsStore> = Arc::new(SqliteContactsStore::new(contacts));
    let objects: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(engine));
    conformance::run(store, objects, uuid::Uuid::now_v7(), uuid::Uuid::now_v7()).await;
}
