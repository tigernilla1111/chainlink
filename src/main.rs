use std::collections::HashSet;

use crate::chain::{
    codec::{deserialize, serialize},
    crypto::{PrivateKey, verify_msg},
    types::{HashBytes, LedgerUpdate, LedgerUpdateType, SignedLedgerUpdate},
};
use crate::network::Interface;

mod chain;
mod network;

#[tokio::main]
async fn main() {
    let mut iface = Interface::new("127.0.0.1:0").await;
    let mut ledger_update_rcvr = iface.subscribe_to_gossips().unwrap();
    let mut node = Node::new();
    while let Some(signed_update) = ledger_update_rcvr.recv().await {
        let SignedLedgerUpdate {
            signature,
            canonical_bytes,
        } = signed_update.clone();

        let update: LedgerUpdate = deserialize(&canonical_bytes).unwrap();

        if verify_msg(&signature, &canonical_bytes, update.author).is_err() {
            println!("ERROR invalid signature on {update:?}");
            continue;
        }

        //validate ledger update against state
        match update.data {
            LedgerUpdateType::Comment {
                parent_post_id,
                parent_comment_id,
                content: _,
            } => {
                // check if post_id and parent_id are valid
                if node
                    .state
                    .validate_comment(parent_post_id, parent_comment_id)
                    .is_err()
                {
                    println!("ERROR comment is not a valid ledger update");
                    continue;
                };
            }
            LedgerUpdateType::Post { content } => {}
        }

        node.mempool.add_ledger_update(signed_update);
    }

    // let block_update_rcvr =
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
}

struct Node {
    mempool: MemPool,
    state: State,
}
impl Node {
    fn new() -> Self {
        Node {
            mempool: MemPool {
                inner: HashSet::new(),
            },
            state: State,
        }
    }
}

/// Requirements:
/// - keep track of each fork, and for each fork keep track of:
///         - the current state
///         - block hashes in order
/// - validate if a block is a valid addition to any of the forks (ie its prev_block_hash is one of the fork heads)
/// - an immutable base state which gets appended to with a block, `block_a`.  `block_a` is only appended when the
///   height reaches `block_a.height + x`
struct State;
impl State {
    fn validate_comment(
        &self,
        parent_post_id: HashBytes,
        parent_comment_id: Option<HashBytes>,
    ) -> Result<(), ()> {
        Ok(())
    }
}
struct MemPool {
    inner: HashSet<SignedLedgerUpdate>,
}
impl MemPool {
    fn add_ledger_update(&mut self, update: SignedLedgerUpdate) {
        self.inner.insert(update);
    }
}

fn sign(update: LedgerUpdate, priv_key: PrivateKey) -> SignedLedgerUpdate {
    let canonical_bytes = serialize(&update).unwrap();
    let (signature, _) = priv_key.sign_msg(&canonical_bytes).unwrap();
    SignedLedgerUpdate {
        signature,
        canonical_bytes,
    }
}
