use crate::agents::{self, Agents};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
};

pub struct AgentUi {
    pub controller: Arc<Agents>,
    pub events: mpsc::Receiver<agents::Event>,
    pub status: agents::Status,
    pub summary: String,
    pub opened: HashMap<u64, u64>,
}
impl AgentUi {
    pub fn new(
        core: Arc<nexssh_core::Core>,
        runtime: &tokio::runtime::Runtime,
        settings: &serde_json::Value,
    ) -> Self {
        let controller = Arc::new(Agents::default());
        let (sender, events) = mpsc::channel();
        let host = agents::Host::new(core, &controller, sender);
        let _entered = runtime.enter();
        controller.init(&host, settings);
        let status = controller.status();
        Self {
            controller,
            events,
            status,
            summary: String::new(),
            opened: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_host_serves_authenticated_mcp_and_stops_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let core = Arc::new(
            nexssh_core::Core::with_secrets(dir.path(), nexssh_core::secrets::Secrets::in_memory())
                .unwrap(),
        );
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let ui = AgentUi::new(
            Arc::clone(&core),
            &runtime,
            &serde_json::json!({"agentsEnabled":true,"agentsPort":port}),
        );
        runtime.block_on(async {
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
            while !ui.controller.status().running {assert!(std::time::Instant::now()<deadline);tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
            let client=reqwest::Client::new();let url=format!("http://127.0.0.1:{port}/mcp");
            let message=serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","clientInfo":{"name":"native-test","version":"1"},"capabilities":{}}});
            assert_eq!(client.post(&url).json(&message).send().await.unwrap().status(),401);
            let token=ui.controller.token_blocking(&core.secrets);
            let response=client.post(&url).bearer_auth(&token).json(&message).send().await.unwrap();assert!(response.status().is_success());
            let response:serde_json::Value=response.json().await.unwrap();assert_eq!(response["result"]["serverInfo"]["name"],"nexssh");
            ui.controller.renew_token_blocking(&core.secrets);
            assert_eq!(client.post(&url).bearer_auth(&token).json(&message).send().await.unwrap().status(),401);
            let new_token=ui.controller.token_blocking(&core.secrets);
            ui.controller.apply(&serde_json::json!({"agentsEnabled":false}));assert!(!ui.controller.status().running);
            // Existing HTTP keep-alive connections are refused as well as new connections.
            if let Ok(response)=client.post(&url).bearer_auth(&new_token).json(&message).send().await {assert_eq!(response.status(),401);}
        });
    }
}
