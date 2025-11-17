use mdns_sd::{ServiceDaemon, ServiceInfo, ServiceEvent};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

const SERVICE_TYPE: &str = "_dsync._tcp.local.";

pub struct PeerDiscovery {
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashMap<String, String>>>,
    // instance name -> address:port
}

impl PeerDiscovery {
    pub fn new() -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        Ok(Self {
            daemon,
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    // Register this instance on the network
    pub fn register_service(&self, port: u16, instance_name: &str) -> anyhow::Result<()> {
        let hostname = hostname::get()?
            .into_string()
            .unwrap_or_else(|_| "dsync-node".to_string());

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            instance_name,
            &hostname,
            "",
            port,
            None,
        )?;

        self.daemon.register(service_info)?;
        println!("Registered mDNS service: {} on port {}", instance_name, port);
        Ok(())
    }

    // Start browsing for other dsync instances
    pub async fn start_browsing(&self) -> anyhow::Result<()> {
        let receiver = self.daemon.browse(SERVICE_TYPE)?;
        let peers = Arc::clone(&self.peers);

        tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let instance_name = info.get_fullname().to_string();
                        let addresses = info.get_addresses();
                        let port = info.get_port();

                        if let Some(addr) = addresses.iter().next() {
                            let peer_addr = format!("{}:{}", addr, port);
                            println!("Discovered peer: {} at {}", instance_name, peer_addr);

                            let mut peers_map = peers.lock().await;
                            peers_map.insert(instance_name.clone(), peer_addr);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        println!("Peer removed: {}", fullname);
                        let mut peers_map = peers.lock().await;
                        peers_map.remove(&fullname);
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    // Getting list of discovered peers
    pub async fn get_peers(&self) -> Vec<String> {
        let peers = self.peers.lock().await;
        peers.values().cloned().collect()
    }
}
