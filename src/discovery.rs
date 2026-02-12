use mdns_sd::{ServiceDaemon, ServiceInfo, ServiceEvent};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, mpsc};
use get_if_addrs::get_if_addrs;

const SERVICE_TYPE: &str = "_dsync._tcp.local.";

pub struct PeerDiscovery {
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashMap<String, String>>>,
    // instance name -> address:port
    my_id: String,
}

impl PeerDiscovery {
    pub fn new(my_id: String) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        Ok(Self {
            daemon,
            peers: Arc::new(Mutex::new(HashMap::new())),
            my_id,
        })
    }

    // Register this instance on the network
    pub fn register_service(&self, port: u16, name: &str) -> anyhow::Result<()> {
        let mut hostname = hostname::get()?
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());

        if !hostname.ends_with(".local") {
            hostname.push_str(".local.");
        }
        let instance_name = format!("{}-{}", name, hostname.trim_end_matches(".local."));

        let mut properties = HashMap::new();
        properties.insert("id".to_string(), self.my_id.clone());

        let my_ips = get_if_addrs()?
            .into_iter()
            .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
            .map(|i| i.ip())
            .collect::<Vec<_>>();

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            my_ips.as_slice(),
            port,
            Some(properties),
        )?.enable_addr_auto();

        self.daemon.register(service_info)?;
        println!("Discovery Active: {} (Port {})", instance_name, port);
        Ok(())
    }

    // Start browsing for other dsync instances
    pub async fn start_browsing(&self, notify_tx: mpsc::Sender<String>) -> anyhow::Result<()> {
        let receiver = self.daemon.browse(SERVICE_TYPE)?;
        let peers = Arc::clone(&self.peers);
        let my_id = self.my_id.clone();

        tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let instance_name = info.get_fullname().to_string();
                        let port = info.get_port();

                        let peer_id = info.get_property("id")
                            .map(|p| p.val_str())
                            .unwrap_or("unknown");

                        if peer_id == my_id { continue; }

                        let mut peers_map = peers.lock().await;
                        if peers_map.contains_key(&instance_name) { continue; }
                        let addresses = info.get_addresses();
                        
                        for addr in addresses {
                            if addr.is_ipv4() {
                                let peer_addr = format!("{}:{}", addr, port);
                                println!("Discovered peer: {} ({})", instance_name, peer_addr);
                                peers_map.insert(instance_name.clone(), peer_addr.clone());
                                let _ = notify_tx.send(peer_addr).await;
                                break;
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let mut peers_map = peers.lock().await;
                        if peers_map.remove(&fullname).is_some() {
                            println!("Peer left: {}", fullname);
                        }
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
