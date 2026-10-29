# Decisions

Newest first. Add an entry in the PR that makes the decision. To reverse one, add a new entry instead of editing the old one.

## Decided

**2026-10-27: At most 4 orphans wait on one missing parent, that parent's oldest dropped first.**
Fills in design §4's "count per parent". An honest missing parent usually has one or two children waiting (a race at one height), so 4 leaves two spare, and one withheld parent holds at most 4 × 128 KB = 512 KB, 1/32 of the 16 MB pool. 3 and 8 were weighed: 3 leaves one spare for the same race, 8 lets one parent hold 1 MB for no honest gain. The cost: a fifth honest child of one parent is dropped and has to be fetched again.

**2026-10-27: The invalid set holds 4,096 block IDs, oldest dropped first.**
Fills in design §3's "capped". About 100 bytes per ID with its set and order entries, so about 400 KB. It only needs the bad blocks whose parent is above the finality horizon: step 4 drops a block whose parent is below it after a lookup. An attacker mines about f × 24 blocks per horizon window, f being their hashrate over the network's, so cycling 4,096 IDs through the set within a window takes about 170× the network's hashrate. Only blocks that passed step 2 are remembered (entry below), so every entry costs about 2.5 s of one core at today's difficulty. 65,536 IDs (about 6 MB) was weighed and would only save re-checks. The cost: past that, a resent bad block is checked again, steps 5 to 7.

**2026-10-27: A block with bad PoW is not remembered as invalid.**
Qualifies the 2026-10-25 timestamp entry's "a block with bad PoW is still remembered as invalid". A wrong difficulty is one compare, and checking the work is one SHA-256 of the header, the same hash the invalid-set lookup needs for the ID, so remembering such a block saves nothing; and since a header with bad PoW costs nothing to make, remembering them would let anyone push all 4,096 real entries out of the set for free. Keeping them was weighed: the set would then only stop casual resends.

**2026-10-25: `tx_root` is an RFC 6962-shaped merkle root over the txs' full encodings, signatures included.**
Without signatures in the leaves, a peer could swap a tx's signature under an unchanged `tx_root` and get an honest block remembered as invalid (design §11). No leaf is duplicated, so two tx lists can't share a root. The cost: an inclusion proof carries the whole tx.

**2026-10-25: A block's timestamp is at most 132 s past the local clock, and after the median of the last 11 blocks.**
132 s (2.2 min, provisional) is about four 30 s target intervals: room for ordinary clock skew, little room to skew Phase 2's retarget. The median of the last 11 timestamps ending at the parent bounds how far back a miner can push them. The clock check runs after PoW, so a block with bad PoW is still remembered as invalid. The caller passes the clock in; `chain` never reads one.

**2026-10-25: The fixed difficulty is `DIFFICULTY` = 2^22 until Phase 2's retarget.**
About 2.5 s a block on one core in release, so local nodes mine at a usable pace before the retarget, while a block still costs real work. Qualifies the 2026-03-01 "30 s blocks" entry until then: 30 s stays the target that `R`, `TX_TTL` and `FINALITY_DEPTH`'s durations assume. Tests never mine (a debug build hashes at about 0.08 MH/s): one header was mined offline and its nonce pinned.

**2026-10-25: A post or comment body is non-empty UTF-8 of at most `MAX_BODY_BYTES`, counted in bytes, not characters.**
Checked in step 6, so the state never sees a malformed body. Deletes have no body.

**2026-10-24: Accounts are forgotten: no account records, the key is the identity.**
Settles open question 4 on its lean. `seen_txs` with a `TX_TTL` expiry blocks replays, so a node rebuilds everything from the blocks within `R`. Invites, if adopted, will need renewing within `R`.

**2026-10-24: Replies to deleted posts and comments are allowed.**
Reverses the "replying to it is invalid" part of the 2026-03-25 comments entry. A reply needs its post within `R` and its `reply_to`, if set, in the same thread; a delete hides a body, it doesn't close a thread. Deletes still need a live target by the same author.

**2026-10-24: The state keeps an `Arc<Block>` for each of its last `R + 1` heights, and `apply_block` takes the whole block.**
Advancing reads the expiring blocks' txs and recomputes their IDs to drop posts, threads and `seen_txs` entries; the store holds the same blocks, so the pointers cost no extra memory, and the state needs no second index of what each block created.

**2026-10-24: The genesis block and its ID are compiled-in constants, and `Tx::id()` takes no argument.**
Every node must agree on them, like `R` and `MAX_BLOCK_BYTES`, and the ID starts every tx's signed bytes. The ID is a hard-coded constant pinned by a test. `BlockStore::new` keeps taking its root block as data. The cost: a second network, such as a test net, needs its own build.

**2026-10-24: `apply_tx` checks the tx height window, and the mempool uses the same check with the best height + 1.**
The window is what makes the `seen_txs` expiry safe, so it lives beside it; block building gets it through `apply_tx`. The upper bound applies on arrival too, so a tx that never expires can't fill the pool. The cost: a tx whose `valid_until_height` is more than `TX_TTL` ahead is rejected on arrival instead of held until it fits.

**2026-10-24: Arrival drops txs the best chain already includes.**
Qualifies the 2026-01 lazy-mempool entry. A tx whose ID is in the best tip's `seen_txs` can't go into a block on that chain, and a reorg away from its block puts it back, so it's dropped on arrival and not relayed. Replays of included txs are free to send, since honest authors signed them, so step 6 alone let anyone make every node hold up to `TX_TTL` blocks of dead txs. Other state checks still wait for block building.

**2026-10-24: Difficulty retargets every block from recent block times, built in Phase 2.**
`R` counts blocks, so posts live 7 days only while blocks average 30 s, and on a network of a few machines one miner leaving can cut hashrate to a tenth. A per-block rule (LWMA-style, weighting recent block times) corrects that within tens of blocks; a Bitcoin-style epoch would leave the rest of a 120-block epoch at 300 s blocks, up to about 10 hours. It's built with Phase 2's mining, when real block times can tune its window; until then difficulty stays fixed. Step 5 will require each block's difficulty to equal the rule's value, and the rule relies on step 2's future limit and step 5's median-time check to bound how far miners can skew timestamps. Settles the "if that matters" in the 2026-03-01 entry below.

**2026-10-23: The header carries a plain `difficulty: NonZeroU64` instead of compact `bits`, and `total_work` no longer saturates.**
Reverses the encoding and saturation parts of the `bits` entry below; the store still derives work from the header. A plain number needs no decoder and has none of compact bits' edge cases: no sign bit, no overflow, no non-canonical encodings, and zero can't be represented, so `work()` can't fail and the store has no bad-bits error. Work is the field itself, so `total_work` is a plain sum, and summing u64 work can't overflow a U256 short of 2^192 blocks, so it no longer saturates. A future per-block retarget rule can output a difficulty directly instead of rounding it to a 23-bit mantissa. The hash is read big-endian, so each 16× increase in difficulty requires about one more leading zero in its hex ID. The header grows from 92 to 96 bytes and still hashes in two SHA-256 blocks; a full 32-byte target would need a third. The cost: difficulty caps at 2^64 − 1 expected hashes per block, about 600 PH/s at 30 s blocks, far beyond what this chain will see.

**2026-10-23: `bits` is Bitcoin's compact target encoding, and the store computes work from it.**
Known values, such as Bitcoin's difficulty-1 `0x1d00ffff`, check the decoder. The store derives `total_work` from the header itself, so nothing outside it can hand it a wrong value, and a block whose `bits` don't decode to a target is rejected. `total_work` saturates instead of overflowing; only `bits` that step 2 rejects can get there.

**2026-10-23: `posts`, `comments` and `threads` are `OrdMap`s, and a thread is a range of keys.**
Measured with 5M random 32-byte keys: imbl's `HashMap` takes 173 B per entry, its `OrdMap` 65, std's `HashMap` 69. Lookups are slower (about 0.7 µs instead of 0.25), still small next to a signature check. A `Vector` of IDs allocates about 2 KB once it holds two, so `threads` keys each comment by `(post, position)` instead of keeping a `Vector` per post. With these types, folding finalized blocks into a plain mutable state would save almost no memory, so the state stays one snapshot per block.

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
2. How spam is limited: identity gating, per-tx PoW, or both. Leaning toward invite-only signup, with one invite per account per period, counted in blocks rather than wall-clock time. Leaning toward getting there in stages: the allow list as a node setting first, then an on-chain allow list edited by a 2-of-3 threshold admin key, then member invites on top of it, each a restart from a new genesis while the network is small.
3. Max post body size. Provisionally 1,024 bytes, about 1,000 characters of English text or 250 emoji. At that size a full block holds about 120 posts.
4. Settled: accounts are forgotten (2026-10-24, above).
5. Recovering from a fork deeper than `FINALITY_DEPTH`. A split longer than 12 minutes (a partition, or a miner that kept mining offline) leaves nodes on different chains for good. Options include an operator command to switch forks, and miners pausing when they have no peers.
6. How a new node finds the real chain without blocks back to genesis: keep every header (about 100 MB a year, no content), or ship checkpoints.
7. How the app and network get updated, including which settings can change without restarting the network. A restart is acceptable but should be rare.

## Assumptions

Single-process node, plain Ed25519 signatures verified strictly, SHA-256 hashes, bincode 2 with fixed-width ints for everything hashed or signed, no light clients.
