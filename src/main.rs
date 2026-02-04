use crate::{
    crypto::PrivateKey,
    network::{Interface, SignedLedgerUpdate},
    types::LedgerUpdate,
    util::{deserialize, serialize},
};

mod crypto;
pub mod network;
mod types;
mod util;

#[tokio::main]
async fn main() {
    println!("Hello, world!");
    let mut iface = Interface::new("127.0.0.1:0").await;
    let mut ledger_update_rcvr = iface.subscribe_to_gossips().unwrap();
    while let Some(update) = ledger_update_rcvr.recv().await {
        let SignedLedgerUpdate {
            signature,
            canonical_bytes,
        } = update;
        let update: LedgerUpdate = deserialize(&canonical_bytes).unwrap();
    }

    // consume the block RPC receiver
    // for blockRpc in block_receiver.recv() {
    //     let BlockRpc {
    //         block_hash,
    //         block_bytes,
    //         version,
    //     } = blockRpc;
    //     // Verify hash(block_bytes) = block_hash
    //     // Deserialize block
    //     // Verify block
    // }

    // // consume the ledgerUpdate receiver
    // for update in ledger_update.recv(){
    // let SignedLedgerUpdateBytes {
    //      signature,
    //      canonical_bytes
    // }
    // // check against state to see if valid
    // }
}

fn sign(update: LedgerUpdate, priv_key: PrivateKey) -> SignedLedgerUpdate {
    let canonical_bytes = serialize(&update).unwrap();
    let (signature, _) = priv_key.sign_msg(&canonical_bytes).unwrap();
    SignedLedgerUpdate {
        signature,
        canonical_bytes,
    }
}
