use super::*;
use tinybus::{broker::Broker, module::ModuleHost, transport::memory::MemoryBus};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test(flavor = "current_thread")]
async fn linked_module_is_admitted_and_serves_search_interface() -> TestResult {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    broker.clone().spawn(bus.clone());
    let host = ModuleHost::new(broker);
    host.attach_linked_with_config(
        crate::linked_module()?,
        serde_json::to_value(SearchConfig::default())?,
    )?;
    let caller = Connection::connect(bus.connect().await?).await?;
    let proxy = caller.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let tools: ListToolsResponse = proxy.call("ListTools", ()).await?;
    assert!(tools.tools.is_empty());
    Ok(())
}

#[test]
fn linked_module_can_be_constructed_repeatedly() {
    // The module keeps no process-global state: each instance builds its own
    // `SearchService` in `setup`, so several can coexist in one process.
    assert!(crate::linked_module().is_ok());
    assert!(crate::linked_module().is_ok());
}
