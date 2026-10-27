# socialchain

A proof-of-work blockchain for short social posts and comments. Nodes gossip transactions and blocks, follow the chain with the most work, and expose the current posts and threads to an app.

Content is ephemeral: nodes drop each post, with its whole thread, `R` blocks (7 days) after it was posted. Anyone can still copy what they see while it's live. There are no coins, fees, or media.

- [docs/design.md](docs/design.md): how it works
- [docs/decisions.md](docs/decisions.md): what was decided and why

If the code and the docs disagree, fix whichever is wrong in the same PR.

## Run

```bash
cargo run
```

Starts a single node on a random local port. The prototype in `src/` is being replaced by Phase 0.

## Roadmap

One branch and PR per item. Tick it when it merges.

**Phase 0: match the repo to the design**

- [x] Design docs
- [x] Create the `chain` module (no networking or disk access)
- [x] Replace `types.rs`, and the placeholder `State` and `MemPool` in `main.rs`, with the types in [design §3](docs/design.md#3-data)
- [x] Switch to SHA-256 and plain Ed25519 with strict verification
- [x] Cap encoding and decoding at `MAX_BLOCK_BYTES`, so a full-size block round-trips

**Phase 1: chain library**

- [x] Block store: insert blocks, track tips, total work, fork point. Test: in a random tree, `tips` is exactly the childless blocks, and the fork point of any two blocks is their lowest common ancestor.
- [x] `ChainState` and `apply_block` on a single chain, including dropping threads past `R`. Test: each validation rule on its own, including the `R` boundary.
- [x] `validate.rs`: the checks that need no state (design §4 steps 2, 5 and 6): PoW and size limits, the timestamp against the local clock and the median of recent blocks (the store checks the height), merkle root, tx format, body size and signatures. Test: each check on its own, including the body-size and timestamp boundaries; a known-answer vector pins the merkle root, and a tx signed for another genesis is rejected.
- [ ] Fork choice and reorg. Test: in a random tree, the best tip has the most work of the tips the node tracks, and its state matches a replay along the best chain.
- [ ] Pruning. Test: a fork whose fork point is just below the horizon is dropped; one whose fork point is at the horizon survives.
- [ ] Mempool and block building. Test: a built block always passes validation, even at the `R` and expiry boundaries, and fits in `MAX_BLOCK_BYTES`; a reorg puts disconnected txs back; under random orders of tx arrivals and blocks, including the same tx in both forks of a reorg, the mempool never holds a tx in the best tip's `seen_txs`; a tx arriving with `valid_until_height` at best + 1 or best + 1 + `TX_TTL` is kept, and one past either end is dropped.
- [ ] Read API. Test: `min_conf = 3` sees exactly the state two blocks below the tip, deletes included.

**Phase 2: two nodes talking**

- [ ] Two local nodes mine, gossip, post, comment, delete, and read
- [ ] Per-block difficulty retargeting from recent block times. Test: in a simulation where hashrate jumps 10× up or down, block time is back within 10% of 30 s after one window of blocks; a block with any other difficulty is rejected.
- [ ] Measure fork rate and fork depth, and set `FINALITY_DEPTH` from them
- [ ] A node that joins late syncs the last `R + FINALITY_DEPTH` blocks

**Phase 3: app**

- [ ] `feed` and `thread` reads
- [ ] Investigate the best way to do a UI and the client API it uses. Current idea: the node serves a web UI that signs txs through a WASM build of `chain`, over JSON-RPC. Compare it with the alternatives, including REST or another transport instead of JSON-RPC.
- [ ] Investigate the security implications of the chosen API: abuse and resource exhaustion from public callers, requests from other sites in a browser, and which methods (mining, peers) must stay local-only
- [ ] CLI or small client. Make it a client of the node's API, not a caller of node internals, and have it sign txs itself so the node never holds user keys. Add a fixed-port option to the node. Then a web UI can come later as a second client of the same API.
- [ ] Split `chain` into its own library crate, once the CLI needs it. Keep it buildable for `wasm32` (no tokio, disk or OS randomness), checked in CI, so a web UI can sign txs with it.
- [ ] Restart cache: keep the in-memory blocks on disk so a restart doesn't resync

**Later, only if needed:**

- Faster block propagation, if forks are common: send the header and tx IDs only, since peers already have the txs in their mempool
- Permissioned miner set, if forks are common: fewer, known miners means fewer races ([open question 1](docs/decisions.md#open))
- Invite-only signup, if spam calls for it ([open question 2](docs/decisions.md#open))
- Lock-free reads, if profiling shows contention

**Much later:**

- Let users break fork ties: when two tips have equal work, show one post from each and mine on the block whose post the user picks. Only the tie-break changes, so most work still wins, and first-seen stays the default if the user doesn't pick in time.
- Earn invites by mining, capped so mining power doesn't turn into social power
- Drop mempool replies and deletes whose post has aged out past `R`, instead of keeping them until `valid_until_height`. Block building already skips them, so this only saves mempool space.

**Not planned:** operator takedowns, keeping content past `R`, media.

## Open questions

See [decisions.md](docs/decisions.md#open).
