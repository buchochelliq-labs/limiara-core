//! The sync loop's futures are `Send`, so an app can run them on a spawned
//! task. A borrowed iterator held across an await breaks this, and only a
//! caller that spawns notices; this test is that caller, at compile time.

use platform_objects::MutationContext;
use platform_sync::{HttpTransport, SyncClient};
use uuid::Uuid;

fn assert_send<T: Send>(_: T) {}

#[test]
fn the_sync_loop_can_run_on_a_spawned_task() {
    // Never called: building it is the test.
    #[allow(dead_code)]
    fn futures(client: &SyncClient<HttpTransport>) {
        let context = MutationContext::new(Uuid::nil());
        assert_send(client.sync_once(context));
        assert_send(client.follow(context, async {}, |_report| {}));
    }
}
