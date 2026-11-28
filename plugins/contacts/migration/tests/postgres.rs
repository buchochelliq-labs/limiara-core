//! The migration between the PostgreSQL Contacts and engine stores, as the
//! server runs it: the platform's application role, forced row-level
//! security, and the Contacts and engine execution contexts.

mod conformance;

use contacts_plugin::store::{ContactsStore, PostgresContactsStore};
use platform_objects::store::postgres::PostgresObjectStore;
use platform_objects::ObjectStore;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn a_postgres_workspace_moves_to_the_engine() {
    let connect = |name: &'static str| async move {
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&std::env::var(name).expect(name))
            .await
            .expect("connect")
    };
    let app = connect("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = connect("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenants = [Uuid::now_v7(), Uuid::now_v7()];
    for tenant in tenants {
        sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Contacts migration')")
            .bind(tenant)
            .execute(&migrator)
            .await
            .expect("tenant");
    }
    let store: Arc<dyn ContactsStore> = Arc::new(PostgresContactsStore::new(app.clone()));
    let objects: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::run(store, objects, tenants[0], tenants[1]).await;
}
