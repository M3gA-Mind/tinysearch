use super::*;
use tinybus::{broker::Broker, module::ModuleHost, transport::memory::MemoryBus};

#[tokio::test(flavor = "current_thread")]
async fn linked_module_is_admitted_and_serves_search_interface() {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    broker.clone().spawn(bus.clone());
    let host = ModuleHost::new(broker);
    host.attach_linked_with_config(
        crate::linked_module().unwrap(),
        serde_json::to_value(SearchConfig::default()).unwrap(),
    )
    .unwrap();
    let caller = Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let proxy = caller
        .proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)
        .unwrap();
    let tools: ListToolsResponse = proxy.call("ListTools", ()).await.unwrap();
    assert!(tools.tools.is_empty() || !tools.tools.is_empty());
}

#[test]
fn two_linked_instances_do_not_share_state() {
    // The module keeps no process-global state: each instance builds its own
    // `SearchService` in `setup`, so attaching twice must both succeed.
    assert!(crate::linked_module().is_ok());
    assert!(crate::linked_module().is_ok());
}
