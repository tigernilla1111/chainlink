use crate::network::Interface;

mod chain;
mod network;

#[tokio::main]
async fn main() {
    let mut iface = Interface::new("127.0.0.1:0").await;
    let mut tx_rx = iface.subscribe_to_gossips().unwrap();
    while let Some(tx) = tx_rx.recv().await {
        println!("received tx from {:?}", tx.author);
    }
}
