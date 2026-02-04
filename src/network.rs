use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::ServerBuilder;
use tokio::net::ToSocketAddrs;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{
    UnboundedReceiver as Receiver, UnboundedSender as Sender, unbounded_channel,
};

use crate::types::SignatureBytes;

const RPC_REQUEST_TIMEOUT_SECS: u64 = 5;

type PubKey = String;

#[rpc(server, client)]
pub trait MyRpc {
    #[method(name = "ping")]
    async fn ping(&self) -> RpcResult<()>;
    #[method(name = "gossip")]
    async fn ledger_update_gossip(&self, msg: SignedLedgerUpdate) -> RpcResult<()>;
    #[method(name = "block_gossip")]
    async fn block_gossip(&self) -> RpcResult<()>;
}

struct Server {
    update_sender: Sender<SignedLedgerUpdate>,
}
impl Server {
    fn new(gossip_sender: Sender<SignedLedgerUpdate>) -> Self {
        Self {
            update_sender: gossip_sender,
        }
    }
}
#[async_trait]
impl MyRpcServer for Server {
    async fn ping(&self) -> RpcResult<()> {
        println!("got ping");
        Ok(())
    }
    async fn ledger_update_gossip(&self, msg: SignedLedgerUpdate) -> RpcResult<()> {
        println!("received ledger update RPC");
        self.update_sender.send(msg).unwrap();
        Ok(())
    }
    async fn block_gossip(&self) -> RpcResult<()> {
        todo!()
    }
}

pub struct Interface {
    addr: SocketAddr,
    gossip_rx: Option<Receiver<SignedLedgerUpdate>>,
    // TODO: move mutable structs into a State protocol
    // Define a lock order on these structs to prevent dead locks
    clients: HashMap<SocketAddr, Arc<HttpClient>>,
    addr_book: HashMap<PubKey, SocketAddr>,
}

impl Interface {
    pub async fn new(addr: impl ToSocketAddrs) -> Self {
        let server = ServerBuilder::default()
            .build(addr)
            .await
            .expect("failed to build server");

        let addr = server.local_addr().expect("failed to get local addr");

        let (gossip_tx, gossip_rx) = unbounded_channel();
        let rpc_module = Server::new(gossip_tx).into_rpc().remove_context();
        let server_handle = server.start(rpc_module);
        tokio::spawn(server_handle.stopped());

        Interface {
            addr,
            gossip_rx: Some(gossip_rx),
            clients: HashMap::new(),
            addr_book: HashMap::new(),
        }
    }
    pub fn subscribe_to_gossips(&mut self) -> Option<Receiver<SignedLedgerUpdateBytes>> {
        self.gossip_rx.take()
    }
    pub async fn add_addr(&mut self, pubkey: PubKey, addr: SocketAddr) {
        self.addr_book.insert(pubkey, addr);
    }
    pub async fn send_gossip(&mut self, rcvr: &PubKey, msg_data: &SignedLedgerUpdateBytes) {
        let Some(&rcvr_addr) = self.addr_book.get(rcvr) else {
            eprintln!("recipient address {rcvr} not found in address book, skipping send");
            return;
        };

        let client = self.connect(&rcvr_addr).await;

        match tokio::time::timeout(
            Duration::from_secs(RPC_REQUEST_TIMEOUT_SECS),
            client.ledger_update_gossip(msg_data.clone()),
        )
        .await
        {
            Ok(Ok(_)) => {
                println!("message sent successfully");
            }
            Ok(Err(e)) => {
                eprintln!("RPC call failed {}", e.to_string());
            }
            Err(_) => {
                eprintln!(
                    "RPC call timed out after {} seconds",
                    RPC_REQUEST_TIMEOUT_SECS
                );
            }
        }
    }

    pub async fn connect(&mut self, addr: &SocketAddr) -> Arc<HttpClient> {
        let client = self.clients.entry(*addr).or_insert_with(|| {
            let server_url = format!("http://{}", addr);
            Arc::new(
                HttpClientBuilder::default()
                    .build(&server_url)
                    .expect("failed to build HTTP client"),
            )
        });
        client.clone()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct SignedLedgerUpdate {
    pub signature: SignatureBytes,
    pub canonical_bytes: Vec<u8>,
}
