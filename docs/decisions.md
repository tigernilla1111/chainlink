# Decisions

Newest first. Add an entry in the PR that makes the decision. To reverse one, add a new entry instead of editing the old one.

## Decided

**2026-10-23: `bits` is Bitcoin's compact target encoding, and the store computes work from it.**
Known values, such as Bitcoin's difficulty-1 `0x1d00ffff`, check the decoder. The store derives `total_work` from the header itself, so nothing outside it can hand it a wrong value, and a block whose `bits` don't decode to a target is rejected. `total_work` saturates instead of overflowing; only `bits` that step 2 rejects can get there.

**2026-10-22: `MAX_BLOCK_BYTES` is 131,072 bytes, checked when encoding as well as decoding.**
128 KB means 128 × 1024, like the 1,024-byte body cap. A block is the largest thing the codec handles, so it rejects anything bigger. bincode only checks its limit when decoding, so `serialize` checks too, and a node can't produce bytes its peers would reject.

**2026-03-25: `min_conf` picks which snapshot a read sees.**
A read with `min_conf = k` sees the best chain's state `k − 1` blocks below the tip. Posts, comments and deletes all follow that one rule, and nothing extra is stored per item. `k` is at most `FINALITY_DEPTH + 1`, since snapshots stop at the horizon. Refines the 2026-01 confirmation-depth entry below: depth is still computed when reading, but a read picks a snapshot instead of checking each item.

**2026-03-25: Comments nest, and authors can delete posts and comments.**
A comment names its root post and, optionally, the comment it replies to. `DeletePost` becomes `Delete` and works on both. A deleted item stays as a marker with no body, so replies to it keep their place; replying to it is invalid. A whole thread is dropped `R` blocks after its post. The chain cost is small: 33 more bytes per reply and one more lookup to validate it.

**2026-03-25: SHA-256 hashes and plain Ed25519, verified strictly.**
SHA-256 gives 32-byte IDs, so work fits in U256, and most CPUs accelerate it. Plain Ed25519 with `verify_strict` makes every node accept exactly the same signatures. The prototype's Ed25519ph is meant for messages too big to hash in one pass, and txs are small. Replaces the SHA-512 assumption.

**2026-03-25: Retention limits describe our node, not a guarantee.**
Anyone can copy content while nodes serve it, so `R` and deletes only mean honest nodes stop serving it. This qualifies "nodes can't choose to keep more" in the 2026-02-08 entry below.

**2026-03-02: `R` = 20,160 blocks (7 days). Replaces the 24 h entry below.**
Block interval and size cap stay at 30 s and 128 KB. Threads stay open for a week. Worst-case content in memory is ≈ 2.6 GB, and a new node may need to download that much to sync, so this targets a desktop or small VPS rather than any machine. A deleted post's body stays on nodes for up to 7 days.

**2026-03-01: 30 s blocks, 128 KB max block size, `R` = 2,880 blocks (24 h).**
A post is pending after about 30 s and settled (3 confirmations) after about 90 s. Capacity is roughly 320 posts per block, or about 10 posts/s. Worst-case content in memory is `R` × 128 KB ≈ 370 MB, so any ordinary machine can run a node.
`R` is a block count. With fixed difficulty, a change in hashrate changes how long posts actually live. If that matters, add difficulty retargeting.

**2026-02-27: One crate for now, with `chain` as a module.**
`chain/` must not touch the network or disk, by convention. Split it into a library crate when a second binary (the Phase 3 CLI) needs it.

**2026-02-16: `FINALITY_DEPTH` is a protocol constant, 24 for now.**
It decides which chain a node will follow, so every node needs the same value. If nodes differed, a reorg deeper than one node's setting would split the network. The real value gets set from fork depths measured in Phase 2, and must be at least the largest `min_conf` any client uses.

**2026-02-08: Post bodies are stored in the transaction.**
`CreatePost { body }` and `CreateComment { parent, body }`, covered by the block's merkle root. Code reads bodies only through `Tx::body()`, so moving to hash-committed bodies later is a type change.
The cost: for `R` blocks, every node holds every body and can't drop one. Moving bodies off-chain later changes the tx format and means restarting from genesis.
Revisit this if content needs to live past `R`, if media is added, or if nodes need to take down content themselves. All three need bodies stored outside the tx, with only a hash on-chain.

**2026-02-08: Nothing is kept past `R`.**
Nodes keep the last `R + FINALITY_DEPTH` blocks in memory and drop the rest. There's no disk storage and nodes can't choose to keep more. This is what makes storing bodies on-chain acceptable. A restart cache on disk can come later; it doesn't change how long anything is kept.

**2026-02-08: Delete is an on-chain tombstone.**
`DeletePost { target }` is valid if the target is in this fork's state, within `R`, and signed by its author. It sets `deleted = true` and reads return no body. The bytes stay in the block until it ages out. Comments under a deleted post stay, so one user can't wipe out other people's replies. Reorgs need no special handling: a delete on the losing fork just isn't in the winner's state.

**2026-01: Comments and deletes can only target posts within `R` blocks.**
Validation never has to look past `R`, so memory is always enough to validate. Threads close after `R` blocks. The alternatives (a permanent index of post IDs, or a merkle accumulator) bring back unbounded state or per-tx proofs.

**2026-01: Fork choice uses total work, not chain length.**
They're the same with fixed difficulty, and total work stays correct if difficulty ever adjusts.

**2026-01: One immutable state snapshot per block, no undo.**
Snapshots share structure, so each one costs only what its block changed. A reorg switches which snapshot is current instead of rolling anything back. Snapshots below the finality horizon are dropped.

**2026-01: Confirmation depth is computed when reading, not stored.**
The library reports how deep a block is. The app decides how deep is enough by passing `min_conf`.

**2026-01: Forks are pruned when their branch point falls below the finality horizon.**
Also capped at `MAX_TIPS` live forks, dropping the lowest-work ones first. Fork length and distance from the tip were considered and rejected ([design §6](design.md#6-pruning)).

**2026-01: The mempool is shared across forks and checked lazily.**
Txs are checked against a fork's state only when building a block on it, and failures are cached per (tx, tip).

**2026-01: Start with a single mutex around the chain.**
Move to a single writer with lock-free reads only if profiling shows contention.

**2026-01: No balances or fees.**
Spam is handled by identity gating and/or per-tx PoW.

## Open

1. Open mining or a permissioned miner set. Leaning permissioned: forks would be rarer, so `FINALITY_DEPTH` and `MAX_TIPS` could be smaller. With no block rewards, open mining may draw little honest hashrate, which makes the chain cheap to out-mine.
2. How spam is limited: identity gating, per-tx PoW, or both. Leaning toward invite-only signup, with one invite per account per period, counted in blocks rather than wall-clock time.
3. Max post body size. Provisionally 1,024 bytes, about 1,000 characters of English text or 250 emoji. At that size a full block holds about 120 posts.
4. Accounts: forget them after `R` like content, or keep a permanent registry. Leaning forget: the key is the identity, the chain keeps no account records, and `seen_txs` with a `TX_TTL` expiry blocks replays, so a new node can rebuild everything from the blocks it gets. Invites, if adopted, would then need renewing within `R`. The design follows the lean for now; settle this before the `ChainState` work.
5. Recovering from a fork deeper than `FINALITY_DEPTH`. A split longer than 12 minutes (a partition, or a miner that kept mining offline) leaves nodes on different chains for good. Options include an operator command to switch forks, and miners pausing when they have no peers.
6. How a new node finds the real chain without blocks back to genesis: keep every header (about 100 MB a year, no content), or ship checkpoints.

## Assumptions

Single-process node, plain Ed25519 signatures verified strictly, SHA-256 hashes, bincode 2 with fixed-width ints for everything hashed or signed, no light clients.
