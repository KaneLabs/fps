# Server Meshing: State of the Art — Technical Research

Last updated: 2026-03-12

---

## Table of Contents

1. [Star Citizen / CIG](#1-star-citizen--cig)
2. [SpatialOS by Improbable](#2-spatialos-by-improbable)
3. [Ashes of Creation / IntrepidNET](#3-ashes-of-creation--intrepidnet)
4. [EVE Online / CCP Games](#4-eve-online--ccp-games)
5. [Hadean / Aether Engine](#5-hadean--aether-engine)
6. [Dual Universe / Novaquark](#6-dual-universe--novaquark)
7. [Epic Games / Unreal Engine](#7-epic-games--unreal-engine)
8. [MSquared / Morpheus](#8-msquared--morpheus)
9. [Glenn Fiedler's Distributed Authority Model](#9-glenn-fiedlers-distributed-authority-model)
10. [Open Source Implementations](#10-open-source-implementations)
11. [Academic Research](#11-academic-research)
12. [CRITICAL: Cross-Server Combat](#12-critical-cross-server-combat)
13. [Deterministic vs Non-Deterministic Physics](#13-deterministic-vs-non-deterministic-physics)
14. [Bandwidth Budgets for Server-to-Server Replication](#14-bandwidth-budgets-for-server-to-server-replication)
15. [Key Takeaways for Implementation](#15-key-takeaways-for-implementation)

---

## 1. Star Citizen / CIG

### Architecture Overview

Star Citizen's server meshing connects multiple game servers ("DGS" — Dedicated Game Servers) that work seamlessly as a single large server. Players move between servers without loading screens or visible transitions. The game went live with Server Meshing v1 in December 2024 (Alpha 4.0 preview) at 500 players per shard.

**Sources:**
- [Server Meshing Q&A (Official)](https://robertsspaceindustries.com/en/comm-link/transmission/18397-Server-Meshing-And-Persistent-Streaming-Q-A)
- [Star Citizen Wiki — Server Meshing](https://starcitizen.tools/Server_meshing)
- [Unofficial Road to Dynamic Server Meshing](https://sc-server-meshing.info/)
- [Star Citizen Wiki — Q&A Comm-Link 18397](https://star-citizen.wiki/Comm-Link:18397/en)
- [CitizenCon 2025 Summary](https://hangarbase.org/news/star-citizen-the-expanded-server-mesh-the-future-of-the-verse-revealed-at-citizencon-2025)
- [HN Discussion](https://news.ycombinator.com/item?id=37307253)

### Replication Layer Components

The Replication Layer is the central nervous system of server meshing. It sits between clients, game servers (DGS nodes), and the persistence layer. The key insight: **replication logic was moved out of the game server into a separate middleware service**.

**Services within the Replication Layer:**

| Service | Role |
|---------|------|
| **Replicant** | Handles network entity streaming and state replication between clients and game servers. "Designed to not run any game logic... no animation, no physics, just network code." |
| **Gateway** | Directs packets between clients and Replicants. Even smaller codebase than Replicant. |
| **Atlas** | Tracks which server has authority over which entities/regions. |
| **Scribe** | Handles write-through to the persistence layer. |
| **EntityGraph** | Graph database storing the state of every network-replicated entity. Acts as crash recovery source. Described as "highly scalable services all the way down." |

**Hybrid Service**: The initial implementation combined Replicant, Atlas, Scribe, and Gateway into a single process for testing core concepts before splitting into individual microservices. EntityGraph is NOT part of the Hybrid Service — it's a separate persistence service.

**Key Design Decision**: The Replication Layer is event-driven, not tick-rate based. It processes packets immediately upon arrival rather than batching per tick.

### Entity Authority

Implemented in early 2020. The core concept:

- Any entity is no longer owned by a single dedicated server
- Multiple DGS nodes exist in the mesh
- One DGS node has **authority** (computes the simulation) for a given entity
- Other DGS nodes have a **client view** (receive-only) of that entity
- There is no hierarchy — just binary "authority or replication"

Authority transfer example: a missile travels across server boundary. The Replication Layer provides the missile entity to both servers. As the missile crosses the boundary, authority swaps — the original server becomes the receiver, the new server becomes the simulator.

### Crash Recovery

**Replicant Node Failure:**
- Clients remain connected to the shard
- Simulation temporarily freezes
- Replication Layer spins up replacement nodes
- Entity state recovered from EntityGraph
- Gateway/DGS nodes reconnected
- Target recovery time: "less than a minute"

**Gateway Failure:**
- Recovery measured in seconds
- Gateway holds no game state — simpler recovery protocol

**Hybrid Service Failure (initial implementation):**
- All clients get 30k errors and kicked to menu
- Same as current single-server crash behavior
- Clients rejoin after replacement Hybrid starts

**Server Crash Ship Recovery:**
- "Heartbeat" system with regular logging to persistence
- Backend recognizes unexpected timeout
- Players can spawn ship intact with cargo, status, and items in pre-crash state

### Static vs Dynamic Server Meshing

**Static Server Meshing** (shipped Dec 2024):
- Fixed server allocation — predetermined which DGS handles which area
- Cannot rebalance at runtime
- Good enough for splitting distinct locations (Stanton system vs Pyro system)

**Dynamic Server Meshing** (in development):
- Constant reevaluation of optimal resource distribution
- Authority transfer between servers based on load
- Dynamic server provisioning from cloud
- If 200 players gather at Orison, mesh automatically spins up additional servers
- Demonstrated live at CitizenCon 2025

### Scale Milestones

- March 2024: 800 concurrent players tested
- September 2024 (Test "B"): First test on new Replication Message Queue
- October 2024 (Test "E"): 1000-player concurrency, extended to 2000
- December 2024: Live at 500 players per shard

### Latency Optimization

- Replication Layer deployed in same datacenters as game servers (sub-millisecond latency between them)
- Parallel spawn queues replace single sequential queue
- Simultaneous entity replication to clients and servers

### Cross-Shard State

Each shard maintains independent databases. Special replication code handles:
- **Player Outposts**: Slowly replicated state across shards (door locked/unlocked syncs, immediate changes don't)
- **Minable Resources**: Unique rocks per shard, but global resource counts replicate across all shards
- **Global Inventory**: Player items in global database, transferable between shards via stow/unstow

### Scalability Concern (from HN discussion)

An engineer identified the O(n^2) problem: "Just updating everyone with what happened in a tick requires 4,000,000 data points a tick at 2,000 players. At 10 Hz that is 40 million updates a second." Star Citizen handles 100k+ entities per system — vastly more complex than EVE's entity model.

### Roadmap

- Q2 2026: Replication Message Queue (RMQ) for improved data flow
- Q3 2026: Large-scale public test events
- Long-term: Dynamic Mesh 2.0, seamless travel between star systems (Pyro, Nyx)

---

## 2. SpatialOS by Improbable

SpatialOS was a distributed game server platform that powered real production games (Worlds Adrift, Scavengers, portions of Dune Awakening). It is the most well-documented server meshing platform to date.

**Sources:**
- [Authority and Interest Docs](https://networking.docs.improbable.io/welcome/spatialos-concepts/authority-and-interest/)
- [Cross-Server RPCs](https://documentation.improbable.io/gdk-for-unreal/docs/cross-server-rpcs)
- [Object Interaction](https://documentation.improbable.io/spatialos-overview/docs/object-interaction)
- [Query-Based Interest](https://documentation.improbable.io/spatialos-overview/docs/query-based-interest)
- [Handing Over Authority](https://documentation.improbable.io/spatialos-overview/docs/handing-over-write-access-authority)
- [Designing Workers](https://docs.improbable.io/reference/13.8/shared/design/design-workers)
- [Understanding Access](https://docs.improbable.io/reference/13.6/shared/design/understanding-access)
- [GDC 2017 Inside SpatialOS](https://ims.improbable.io/insights/video-a-look-inside-spatialos-gdc-2017/)
- [GameDev.net Discussion: What happened with SpatialOS](https://www.gamedev.net/forums/topic/703568-whatever-happened-with-spatial-os/)

### Worker Model

SpatialOS distributes game simulation across **workers** — server processes that each simulate a portion of the game world. Multiple worker types can coexist:

- **Server-workers**: Run game logic, physics, AI for their assigned region
- **Client-workers**: Run on players' machines, send input, receive state
- **Specialized workers**: E.g., a "flocking worker" dedicated to bird simulation

Each entity in the world has components. **Authority is per-component, not per-entity.** A single entity can have different components controlled by different workers.

### Authority System

**Authority States** (worker transitions between these):

```
NotAuthoritative → Authoritative → AuthorityLossImminent → NotAuthoritative
```

- Only one server-worker at a time can have write authority over a component
- The Runtime enforces this — never more than one authoritative writer
- Authority is governed by **Access Control Lists (ACLs)** on each entity
- ACLs specify which worker types may read and which may write

**Authority Loss Notification**: When a worker is about to lose authority, it receives `AuthorityLossImminent` callback. This allows the worker to send final important updates before losing write access.

**On transition to Authoritative**: worker receives component update BEFORE the transition
**On transition to NotAuthoritative**: worker receives component update AFTER the transition

### Interest Management (Authority Radius vs Read Radius)

**Legacy system (chunks):** World was overlaid with a square grid. Each client checked out nearby chunks.

**Modern system (Query-Based Interest / QBI):**
- Each worker defines interest based on the components it has authority over
- Interest maps component IDs to a list of queries
- A server-worker might have interest in "every object within a 100m radius of the Actors it has authority over"
- The Runtime provides each worker with only the specific component data it needs

**Key distinction:**
- **Authority area** = the region where a worker has write access (can modify entities)
- **Interest area** = the region where a worker receives updates (read-only view of entities it doesn't own)
- Interest area is typically LARGER than authority area — a worker needs to see entities approaching its boundary before they arrive
- In overlap regions between worker authority areas, it's valid for either worker to have authority

### Cross-Worker Entity Interaction

This is the hardest problem in server meshing. SpatialOS solved it with two mechanisms:

**1. Commands (Request-Response Pattern):**
- Sending worker invokes a command on a component of an entity
- SpatialOS routes the command to whichever worker has write authority over that component
- The sending worker's bridge determines the target worker
- The target worker's bridge receives the request, invokes the handler, and returns a response
- Response routes back through both bridges to the sender

**2. Cross-Server RPCs (GDK for Unreal):**
- When a server-worker invokes an RPC on an Actor another worker has authority over
- The SpatialOS Runtime identifies the authoritative worker
- Passes execution of that RPC to the authoritative worker
- The authoritative worker executes the RPC

This means: **a worker cannot directly modify an entity it doesn't have authority over. It must send a request to the authoritative worker.**

### Load Balancing

Workers are assigned authority regions. Load balancing can:
- Change which worker has authority over a component
- Respond to entity movement into another worker's area
- React to load imbalances

When load balancing triggers authority transfer, the worker receives `AuthorityLossImminent` before losing authority.

### World's Adrift Post-Mortem (Lessons Learned)

World's Adrift was the flagship SpatialOS game. It shut down in 2019.

**Key technical lessons:**

1. **Backward compatibility crisis**: A fundamental change in SpatialOS architecture left the team stuck on an older version. Moving to the new one "would have required rewriting more than 40 percent of their code base" — specifically, "the break in retrocompatibility was such that they would have to rewrite the server-side code in a different language."

2. **Physics complexity**: "Almost every item had its own physics — characters swinging from grappling hooks had their own momentum, ships were assembled piece by piece and broke apart piece by piece, trees fell when cut down." This proved computationally devastating across distributed workers.

3. **Development overhead**: "All their work went into making the game work rather than making it the experience they wanted it to be."

**Takeaway**: Distributed physics across server boundaries is an extraordinarily hard problem. The platform itself worked, but the overhead of working within its constraints consumed the development team.

---

## 3. Ashes of Creation / IntrepidNET

Intrepid Studios built their own server meshing on top of Unreal Engine.

**Sources:**
- [Ashes of Creation Wiki — Server Meshing](https://ashesofcreation.wiki/Server_meshing)
- [IntrepidNET Wiki](https://www.ashesofcreation.wiki/Intrepid_Net)
- [MMORPG.com — Server Meshing Stream](https://www.mmorpg.com/news/ashes-of-creation-stream-breaks-down-new-server-meshing-network-technology-2000132119)
- [Technical Exploration (Nephi Labs)](https://en.nephi-labs.com/2024/07/12/technical-exploration-of-ashes-of-creations-server-meshing/)
- [Alpha Two Preview Discussion](https://forums.ashesofcreation.com/discussion/59653/feedback-request-alpha-two-server-meshing-technology-preview-shown-in-june-livestream/p4)

### Architecture

- A realm is made up of many game servers (not one monolithic server)
- Each server has its own **multi-threaded replicator**
- The **IntrepidNET replication graph** optimizes network communication per-player based on data relevancy
- Inter-server replication seamlessly replicates between servers within a realm

### Proxy Actor System (Cross-Boundary Interaction)

This is their key innovation for cross-boundary interaction:

1. As an actor approaches a server boundary, the server negotiates with the neighbor
2. The neighbor spawns a **"proxy"** of the actor — looks and acts just like the real actor
3. The owning server replicates actor data to the neighboring server
4. Players on both sides of the boundary can interact with each other

**Cross-boundary PvP**: "It is entirely possible for players to engage in PvP while being on opposite sides of the server border." These inter-server communications use Unreal Engine RPCs.

### Authority Transfer ("Promotion")

When players cross a server boundary, a "promotion" system seamlessly transfers authority. The process is invisible to players.

### Dynamic Gridding (Heat Map Load Balancing)

Server boundaries are NOT fixed. Dynamic gridding works via:
- **Heat collection** across the entire server
- Heat is generated by traffic volume and server slowness
- The heat map drives decisions on when splits occur
- If a large PvP battle happens, servers reallocate so the battle is split among several servers
- Relatively empty areas consolidate into a single server

### Performance (2024)

IntrepidNET implemented a **Multithreaded Replication Graph**, increasing replication performance by 2.4x and reducing replication time by 94%.

### Crash Recovery

Game servers that crash are replaced automatically. Players connected to the crashed server relog without affecting players on other servers in the realm.

---

## 4. EVE Online / CCP Games

EVE Online is the longest-running single-shard MMO (since 2003). Their approach is fundamentally different — they don't do server meshing. They do **node-per-solar-system** with Time Dilation.

**Sources:**
- [Tranquility Tech IV (Official)](https://www.eveonline.com/news/view/tranquility-tech-iv)
- [High Scalability — 7+1 Ways EVE Scales](https://highscalability.com/7-sensible-and-1-really-surprising-way-eve-online-scales-to/)
- [Introducing TiDi (Official)](https://www.eveonline.com/news/view/introducing-time-dilation-tidi)
- [TiDi Follow-Up (Official)](https://www.eveonline.com/news/view/time-dilation-hows-that-going)
- [Stackless Python in EVE (Slides)](https://www.slideshare.net/Arbow/stackless-python-in-eve)
- [CCP CTO Presentation (Slides)](https://slideplayer.com/slide/2443961/)
- [PC Gamer — 10,000-player battle experiment](https://www.pcgamer.com/how-eve-onlines-experimental-10000-player-battle-could-radically-change-its-future/)

### Tranquility Cluster Architecture (Tech IV, Current)

**Hardware:**
- Lenovo ThinkSystem SN550 v2 dual-CPU machines
- 2x Intel Xeon Gold 6334 (3.60 GHz, 8 cores each = 16 cores/machine)
- 512 GB DDR4 at 3200 MHz per machine
- 18 machines in 5 FLEX chassis (+ 1 spare chassis)
- Each machine runs ~13 nodes
- **Total: 195 active nodes, 39 spare nodes**

**Database Servers:**
- 2x Dell EMC PowerEdge R750 (primary/standby)
- 2x Intel Xeon Gold 6346 CPUs
- 4 TB DDR4 memory per machine
- IBM Flash System 7200 with NVMe

**Software Stack:**
- Stackless Python (cooperative multitasking via tasklets and channels)
- MS SQL Server for persistence
- Windows-based

### Node Distribution

- **170 nodes** assigned to general solar system simulation (Empire, Null, Wormhole)
- **Jita** (trade hub): solo on one dedicated node
- **The Forge market**: solo on another dedicated node
- **Character Services**: 40 dedicated nodes, ~2 TB total memory

### 8 Scaling Strategies

1. **Do Nothing** — most of the universe is manageable
2. **Run It Hot** — operate servers at 100% CPU
3. **Sharding by Solar System** — each solar system is a process, multiple systems per node
4. **Node Migration** — move solar systems between machines when overloaded; smaller games relocate to free resources
5. **Supernodes ("Reinforced Nodes")** — anticipated large battles get deployed on superior hardware
6. **Operation Throttling** — limit session changes to max one per 10 seconds per character
7. **Brain-in-a-Box** — dedicated nodes for tracking player skills/ship status, sending consolidated updates
8. **Time Dilation (TiDi)** — slow the game clock when overloaded

### Time Dilation (TiDi) — Technical Details

TiDi is NOT lag. It's deliberate game clock manipulation:

- When a node can't keep up with updates, it slows the simulation
- Minimum TiDi: 10% of normal speed (1 second of sim = 10 seconds of real time)
- "By slowing down time the game can process more missiles so clients can be kept in sync, and everyone is still getting an accurate view of the game"
- TiDi affects an entire **node** (the hardware hosting one or more solar systems)
- There is no per-solar-system clock — that would require "fundamental changes in the software architecture"

**Typical large battle flow:**
1. Attacking fleet warps in → server overloads
2. Game clock dilates to ~5% of real time
3. As tasks complete, dilation increases to ~30%
4. Battle plays out slowly but accurately
5. Everyone sees the same game state

### Cross-System Interaction

EVE does NOT do server meshing. A player's session is on one node at a time. Jumping between systems means a session handoff to a new node (with a loading tunnel). The PROXY/SOL split:
- **PROXY nodes**: Handle session management, some processing (e.g., market proxy)
- **SOL nodes**: Run solar system simulation

### Why EVE's Approach Matters

EVE's architecture can be described in one sentence: "each system is a process, and players connect to proxy servers that keep track of which system they're on." It's been running for 20+ years. The simplicity is the feature. TiDi is philosophically the opposite of server meshing — instead of adding more servers to handle load, you slow down time to match available compute.

---

## 5. Hadean / Aether Engine

Hadean (now "Hadean Simulate") is a distributed spatial simulation platform. They partnered with CCP Games and Minecraft.

**Sources:**
- [Hadean Simulate](https://hadean.com/aether-engine/)
- [Aether Engine Datasheet](https://hadean.com/resources/aether-engine-datasheet/)
- [Hadean + CCP Games (Microsoft)](https://developer.microsoft.com/en-us/games/articles/2020/08/hadean-helps-ccp-games-realize-its-vision-with-azure/)
- [Minecraft + Hadean](https://www.pcgamer.com/minecraft-is-using-a-spatial-simulation-engine-to-make-larger-and-more-immersive-experiences/)
- [Hadean Architecture](https://hadean.com/project/massive-scale-simulation-hadean-architecture/)
- [Hadean GitHub](https://github.com/hadeaninc)
- [INN Critical Analysis](https://imperium.news/hadeans-jumping-the-shark/)

### Technical Architecture

- Uses a **distributed octree** data structure to subdivide 3D space across CPU cores
- Dynamically allocates more cores to areas of high compute density
- Scales onto (and off) additional machines automatically
- Can dynamically allocate Azure resources within approximately **50 milliseconds**

### Entity Management

- Entities are managed within the octree spatial partition
- When using HLA/DIS protocols, Hadean Simulate takes ownership of entities via a bridge
- Manages all entities in a single simulation with the ability to broadcast data using any protocol to any endpoint

### Platform Model

The Hadean Platform is described as a "cloud-native operating system implementing a distributed process model." Applications are distributed by default. It dynamically scales by splitting computational tasks and allocating them to CPUs in cloud systems.

### Hadean Connect

A separate scalable compute layer that offsets the connection handling load from the simulation. Traditional servers typically limit connected users to 100-200 before crashing due to CPU constraints — Hadean Connect addresses this by separating connection handling from simulation.

### Limitations

The technical documentation is behind authentication. Public information is mostly marketing-level. The INN analysis raises concerns about whether the technology delivers on its promises in actual game contexts.

---

## 6. Dual Universe / Novaquark

Dual Universe shipped with their own single-shard technology called CSSC (Continuous Single-Shard Cluster).

**Sources:**
- [Dual Universe Wikipedia](https://en.wikipedia.org/wiki/Dual_Universe)
- [Novaquark Explains Server](https://mmos.com/news/novaquark-explains-how-dual-universes-server-works)
- [LinkedIn Deep Dive](https://www.linkedin.com/pulse/dual-universe-dive-fully-editable-continuous-why-matters-baillie)

### Architecture

- All players in a single universe, no loading screens
- Server software dynamically splits clusters of players into **cube-shaped shards** based on location
- Load distributed across multiple servers

### Bandwidth Optimization via Distance

This is a key technique worth studying:

- Players within **100 meters**: full update rate, smooth movement
- Players at **700 meters**: significantly reduced update rate, "blink" in and out
- **Interpolation** techniques smooth movement despite fewer updates

This is essentially **distance-based interest management** built into the core architecture.

### Voxel Considerations

- 25cm precision voxel technology for environment modification
- Unlimited LOD for planet-sized constructions
- The voxel state represents enormous data volumes that must be managed per-server

---

## 7. Epic Games / Unreal Engine

Epic does NOT have a public server meshing product. Fortnite uses traditional sharded dedicated servers (50-100 players per shard). However, there are relevant implementations.

**Sources:**
- [Fortnite Distributed Systems](https://michaeleakins.com/insights/building-for-100m-players-fortnite-distributed-systems/)
- [Fortnite + Kubernetes](https://www.serverwatch.com/server-news/how-epic-games-uses-kubernetes-to-power-fortnite-application-servers/)
- [AWS Fortnite Case Study](https://pages.awscloud.com/fortnite-case-study.html)
- [UE5 Networking Docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/networking-and-multiplayer-in-unreal-engine)

### Fortnite Architecture

- Runs on AWS with Kubernetes (EKS)
- Each game server handles 50-100 players (a "shard")
- No player-to-player connections — all traffic through authoritative servers
- Backend: microservices in Java/Scala, RESTful + C++ clients
- 5,000+ Kinesis shards processing 125M events/minute
- K8s adoption: 40% reduction in idle instances, 25% cost reduction

**Fortnite does NOT do server meshing.** It scales horizontally by running thousands of isolated 100-player instances.

### Jari Senhorst's UE Server Meshing Prototype

A developer built a working server meshing prototype in Unreal Engine. This is the most detailed open implementation documented.

**Source:** [jarisenhorst.com/project/servermeshing](https://jarisenhorst.com/project/servermeshing)

**Architecture:**
- Game Clients (Unreal Engine)
- Dedicated Game Servers (DGS) — simulate world areas
- **Replication Server** — central authority manager (C++ console app)
- **Persistent Database** — Neo4j stores world state

**Streaming Containers (Spatial Partitioning):**
- Hierarchical virtual spatial volumes defining simulation domains
- Replication server assigns DGS to root containers
- Authority begins at root container and applies to all entities within
- Nested containers assigned to different servers create authority boundaries

**Authority Transfer (3-state transition):**
1. **Established** — current server has authority
2. **Transferring** — window where "crucial data like input and latest state are directly forwarded to the new server"
3. **Established (new server)** — old server stops sending updates

**Entity Synchronization:**
- Clients capture input on fixed ticks synchronized between client/server
- Servers apply input, simulate physics, broadcast authoritative state
- Clients store states in circular buffer indexed by server tick
- During render, clients interpolate using alpha calculated from "discrepancy between local/authoritative state and state age"

**Crash Recovery:**
- If authoritative server goes offline, replication server "immediately switches over to the correct server instead of transitioning"
- If all game servers crash but replication server survives, "clients remain connected and the gamestate remains saved"
- On recovery, replication server "seeds" current state to reconnected server

**Communication Model:**
- No client connects directly to game servers — all through replication layer
- Replication server collects entity states from authoritative servers each fixed tick
- Distributes states "to all relevant clients" and "all relevant DGSes (with the exception of the authoritative server)"

**Challenges Overcome:**
1. Unreal's Chaos physics couldn't run in fixed-timestep — switched to state interpolation
2. Input handling across authority transfers required direct forwarding during transition windows
3. Nested authority (servers simulating zones within other servers' domains)

---

## 8. MSquared / Morpheus

Morpheus is a networking replacement for Unreal Engine that claims to support thousands of concurrent players.

**Source:** [Morpheus Networking Docs](https://docs.msquared.io/creation/unreal-development/getting-started/networking/networking)

### Architecture

- Replaces Unreal's standard networking entirely
- Single server coordinates multiple clients
- Claims to enable "thousands of players and objects networked together, using the bandwidth of a standard battle royale game"

### Network Levels (Tiered Replication)

Each client maintains actors at three replication depths:
- **Background**: minimal replication
- **Midground**: moderate replication
- **Foreground**: full state synchronization

This selective replication is likely their bandwidth reduction mechanism.

### Key Design Decisions

- **Universal visibility**: "Each client sees every MorpheusActor in the world; there is no concept of 'net relevancy'"
- Server-centric spawning: only servers spawn/destroy networked actors
- Actors can have immutable authoritative clients

### Limitation

Documentation provides no quantitative bandwidth analysis or specific technical mechanisms for how thousands of players are supported.

---

## 9. Glenn Fiedler's Distributed Authority Model

Glenn Fiedler (Gaffer On Games, formerly at Respawn/Oculus) is one of the most respected voices in game networking. His research on distributed authority for physics is directly relevant to server meshing.

**Sources:**
- [Networked Physics in VR](https://gafferongames.com/post/networked_physics_in_virtual_reality/)
- [State Synchronization](https://gafferongames.com/post/state_synchronization/)
- [Introduction to Networked Physics](https://gafferongames.com/post/introduction_to_networked_physics/)
- [GDC 2015 — Networking for Physics Programmers](https://archive.org/details/GDC2015Fiedler)
- [Choosing the Right Network Model](https://mas-bandwidth.com/choosing-the-right-network-model-for-your-multiplayer-game/)

### Three Approaches to Networked Physics

1. **Deterministic Lockstep** — send only inputs, require bitwise determinism
2. **Snapshot Interpolation** — send full state snapshots, interpolate on client
3. **State Synchronization** — send both input and state, run sim on both sides

### Distributed Authority Model

Fiedler's model for VR (sponsored by Oculus):

- Players **take authority over objects they interact with**
- Authority cascades: "a cube thrown by player 2 could take authority over any objects it interacted with, and in turn any objects those objects interacted with, recursively"
- No dedicated server needed
- Hides latency: if you're the authority, you don't experience lag

### Conflict Resolution via Sequence Numbers

Two sequence counters per object:

**Authority Sequence**: Increments when a player assumes authority OR when an authority-controlled object reaches rest.

**Ownership Sequence**: Increments when a player grabs an object. **Ownership is stronger than authority** — "if a player interacts with a cube just before another player grabs it, the player who grabbed it wins."

The host acts as arbiter, accepting or rejecting guest updates. "Conflicts requiring corrections are rare in practice even under significant latency, and when they do occur, the simulation quickly converges to a consistent state."

### Bandwidth Numbers

- ~**256 kbps per player** (1 Mbps total for 4 players)
- Achieved via:
  - Lossless at-rest encoding (single bit instead of 6 floats)
  - Quantization to 1/1000th centimeter
  - Priority accumulator for selective updates
  - Delta compression with ballistic prediction (~90% perfect prediction rate)

### State Synchronization Details

- Max 64 state updates per packet
- Multiple redundant inputs included
- Jitter buffer on receiver: recommended 4-5 frames at 60Hz
- **Critical**: "quantize the entire simulation state on both sides as if it had been transmitted over the network" — ensures both sides extrapolate from identical quantized values
- Visual smoothing: adaptive error reduction (0.95 factor for errors under 25cm, 0.85 for larger)

### Limitation

"This approach is best suited for cooperative experiences rather than competitive ones requiring server-authoritative security."

**This is the fundamental tension in server meshing: distributed authority enables scale but weakens security.**

---

## 10. Open Source Implementations

### Hajis23/room-game
**Source:** [GitHub](https://github.com/Hajis23/room-game)

Prototype of multi-server real-time online multiplayer with simple server meshing / spatial partitioning between server nodes.

### GoWorld
**Source:** [GitHub](https://github.com/xiaonanln/goworld)

Scalable distributed game server engine in Go with hot swapping. Supports spaces & entities with AOI (Area of Interest). Distributed across multiple machines.

### NoahGameFrame
**Source:** [GitHub](https://github.com/ketoo/NoahGameFrame)

Distributed game server framework for C++ (MMO RPG/MOBA). Actor-based with network library.

### Colyseus
**Source:** [colyseus.io](https://colyseus.io/)

Node.js framework that can distribute rooms across multiple processes or machines. MIT licensed. Not true server meshing but supports horizontal scaling.

### Agones
**Source:** [agones.dev](https://agones.dev/site/)

Kubernetes-based multiplayer dedicated game server orchestration. Handles lifecycle management, not spatial distribution. Used for scaling instances, not meshing.

---

## 11. Academic Research

### Key Papers

**"High-Level Development of Multiserver Online Games"** (Glinka et al., 2008)
- [Wiley](https://www.hindawi.com/journals/ijcgt/2008/327387/)
- Analyzes zoning, instancing, and replication as distribution mechanisms
- Segments can be reallocated to new servers at runtime
- Inter-server client migrations are seamless
- Introduces "active entity" (authoritative) vs "shadow entity" (replica) terminology

**"A Distributed Architecture for MMORPG"** (Assiotis & Tzanov, 2006)
- [ACM](https://dl.acm.org/doi/10.1145/1230040.1230067) / [MIT PDF](https://pdos.csail.mit.edu/archive/6.824-2005/reports/assiotis.pdf)
- Hybrid Client-Server/P2P architecture
- Splits virtual world into smaller regions per server
- Introduces **Game Connection Handoff** in the OS: transparently hands off a client's live game connection between servers
- Addresses: seamless migration, server crash protection, dynamic load balance

**"A Distributed Multiplayer Game Server System"** (UBC)
- [Summary](https://www.cs.ubc.ca/~krasic/cpsc538a/summaries/38/)
- Proposes Mirrored Server architecture
- Trailing state synchronization
- Low-latency reliable multicast protocol

**"A Survey and Taxonomy of Latency Compensation Techniques for Network Computer Games"** (ACM Computing Surveys, 2022)
- [ACM](https://dl.acm.org/doi/10.1145/3519023)
- Comprehensive taxonomy of latency compensation across game types

**"Dynamic Adaptation of User Migration Policies in DVEs"** (IEEE, 2017)
- [IEEE Xplore](https://ieeexplore.ieee.org/document/7966364/)
- Uses Reinforcement Learning to minimize average system response time
- Adaptive distributed user migration policy

**"A Virtualization-Based Approach for Zone Migration in DVEs"** (DISIO/ACM, 2012)
- [EUDL](https://eudl.eu/doi/10.4108/icst.simutools.2011.245557)
- Live zone migration over WANs
- Supports DVE zone re-mapping and load re-distribution

### Key Concepts from Literature

**Three parallelization strategies** (from Glinka 2008):
1. **Zoning** — large user numbers across a large world
2. **Instancing** — many game areas running independently in parallel
3. **Replication** — high user density for action/PvP games

**Area of Interest (AoI)**: Reduces bandwidth by only sending events to entities in range. Server doesn't send combat events to players out of range.

**Overlapping Zones**: The standard academic approach to seamless migration uses overlapping boundary regions where both servers simulate entities. Authority transfers when an entity crosses the center of the overlap zone.

---

## 12. CRITICAL: Cross-Server Combat

This is the hardest unsolved problem in server meshing. Here's how every known system handles it:

### The Problem

Player A is on Server 1. Player B is on Server 2. Player A shoots Player B. Who validates the hit? Who applies the damage? What about latency between the servers?

### Approach 1: Proxy Actors (Ashes of Creation)

- Each server near a boundary spawns **proxy actors** for entities on adjacent servers
- Proxy actors receive replicated state from the authoritative server
- When Player A shoots toward Player B, the RPC goes through the inter-server communication layer
- The server authoritative over Player B processes the hit
- Uses Unreal Engine RPCs for inter-server communication

**Latency concern**: The proxy actor's position is always slightly behind the authoritative position. The gap equals the inter-server replication latency.

### Approach 2: Replication Layer Mediation (Star Citizen)

- The Replication Layer provides entity state to all relevant servers
- Bullets are spawned on each client and server node independently
- Only one server has authority over a given bullet/entity
- As bullets cross boundaries, authority transfers
- Hit validation happens on the authoritative server

**Latency concern**: Bullets spawned client-side on all machines means visual feedback is fast, but authoritative hit detection happens on the server with authority over the target.

### Approach 3: Command Routing (SpatialOS)

- Worker cannot modify an entity it doesn't own
- Must send a **command request** to the authoritative worker
- Command routed through SpatialOS Runtime → target worker's bridge → handler executes → response returns
- The authoritative worker decides the outcome

**Latency concern**: Command round-trip through the Runtime adds latency. For fast combat, this is noticeable.

### Approach 4: Authority Expansion (General Strategy)

When two entities near a boundary interact:
1. Temporarily expand one server's authority to cover both entities
2. Process the interaction on a single server (avoiding cross-server issues entirely)
3. Return authority when interaction completes

This is essentially what Star Citizen does when entities approach boundaries — the Replication Layer can assign authority so that interacting entities share a server.

### Approach 5: Time Dilation (EVE Online)

Don't split the combat — slow it down instead. If a solar system has 6000 players, run at 10% speed rather than trying to split across servers.

**This only works for EVE's turn-based-like combat. Impossible for FPS.**

### Lag Compensation Across Server Boundaries

Standard lag compensation (Valve's technique): the server rewinds time based on the shooter's latency to see what they saw when they fired.

**Cross-server complication**: Server 1 needs to rewind Server 2's entity state. This requires:
- Server 1 to maintain a history buffer of Server 2's entity positions
- Those positions are already delayed by inter-server replication latency
- The rewind must account for: client latency + inter-server latency

**No production system has publicly documented cross-boundary lag compensation.** This appears to be handled by:
1. Making boundary regions wide enough that cross-boundary combat is rare
2. Temporarily transferring authority to avoid the cross-boundary case
3. Accepting slightly degraded hit registration near boundaries

### Known Solutions Summary

| Approach | Latency Impact | Complexity | Used By |
|----------|---------------|------------|---------|
| Proxy actors + RPC | +inter-server RTT | Medium | Ashes of Creation |
| Replication layer routing | +routing hop | High | Star Citizen |
| Command routing | +runtime mediation RTT | Medium | SpatialOS |
| Authority expansion | Minimal (avoids cross-server) | High | General pattern |
| Time dilation | None (slows time instead) | Low | EVE Online |
| Client-side prediction + server reconciliation | +replication delay | Medium | Most approaches |

---

## 13. Deterministic vs Non-Deterministic Physics in Server Mesh Contexts

**Sources:**
- [Gaffer — Deterministic Lockstep](https://gafferongames.com/post/deterministic_lockstep/)
- [SnapNet — Lockstep Architecture](https://www.snapnet.dev/blog/netcode-architectures-part-1-lockstep/)
- [Choosing the Right Network Model](https://mas-bandwidth.com/choosing-the-right-network-model-for-your-multiplayer-game/)

### Why Determinism Matters for Server Meshing

If physics were deterministic, servers could synchronize by sharing only inputs. Two servers simulating the same region would arrive at identical results. This would dramatically reduce inter-server bandwidth.

### Why Nobody Uses Deterministic Physics for Server Meshing

1. **Floating point non-determinism**: Different CPUs, compilers, and optimization levels produce different floating-point results. Achieving bitwise determinism across heterogeneous server hardware is extremely difficult.

2. **Physics engine limitations**: Neither PhysX, Havok, nor Jolt guarantee deterministic results across platforms. Unity's DOTS physics has experimental determinism support.

3. **Scale problem**: Deterministic lockstep requires ALL inputs before advancing. In a server mesh with potentially thousands of entities across many servers, waiting for global input synchronization would add unacceptable latency.

4. **Partial simulation**: In server meshing, each server only simulates its region. Deterministic lockstep assumes all participants simulate everything.

### What's Actually Used

**State synchronization** (send both input and state) is the universal choice for server meshing:
- Doesn't require determinism
- Each server is authoritative for its region
- State replication handles divergence
- Bandwidth cost is higher but manageable with interest management

---

## 14. Bandwidth Budgets for Server-to-Server Replication

### Known Numbers

| System | Inter-Server Bandwidth | Notes |
|--------|----------------------|-------|
| Glenn Fiedler (VR) | ~256 kbps per player | 4-player cooperative scenario |
| Star Citizen | Sub-millisecond latency (same datacenter) | Bandwidth figures not public |
| SpatialOS | Per-component, QBI-filtered | Only sends data workers have interest in |
| Dual Universe | Distance-based degradation | Full rate at 100m, reduced at 700m |
| EVE Online | N/A | No inter-server replication (node-per-system) |

### Strategies for Reducing Inter-Server Bandwidth

1. **Interest management / QBI**: Only replicate entities that neighboring servers need to know about
2. **Distance-based update rate**: Full rate for nearby entities, reduced rate for distant
3. **At-rest optimization**: Don't replicate stationary objects continuously
4. **Delta compression**: Only send what changed
5. **Quantization**: Reduce precision to reduce packet size
6. **Priority accumulator**: Send highest-priority entity updates first, defer lower-priority
7. **Ballistic prediction**: Don't send state if the receiver can predict it accurately (~90% prediction success rate per Fiedler)

### The O(n^2) Problem

From the HN discussion on Star Citizen: "Just updating everyone with what happened in a tick requires 4,000,000 data points a tick at 2,000 players. At 10 Hz that is 40 million updates a second."

This is why interest management is mandatory. Without it, inter-server bandwidth scales quadratically with entity count.

---

## 15. Key Takeaways for Implementation

### What Actually Ships

1. **Star Citizen**: Shipped static meshing with Replication Layer. Working on dynamic. 500-2000 players per shard tested. Real cross-boundary combat exists.

2. **Ashes of Creation**: Proxy actor system with dynamic gridding. Cross-boundary PvP demonstrated. Heat map-based load balancing.

3. **EVE Online**: No meshing — TiDi instead. 20+ years of proven stability. Sometimes the right answer is "don't mesh, slow down."

4. **SpatialOS**: The most well-documented platform. Component-level authority, QBI interest management, cross-server RPCs. Platform itself worked but imposed heavy development overhead.

5. **Dual Universe**: CSSC with cube-shaped shards. Distance-based update rates. Shipped but with performance issues.

### Architecture Patterns That Work

1. **Separate replication from simulation**: Move networking code out of game servers into a dedicated Replication Layer (Star Citizen, Jari Senhorst prototype)

2. **Event-driven, not tick-based replication**: Process packets on arrival, don't batch per tick

3. **Binary authority model**: One server is authoritative, all others are receivers. No hierarchy.

4. **Interest-based filtering**: Only replicate entities that other servers/clients need to know about

5. **Proxy actors for boundary regions**: Spawn lightweight representations of cross-boundary entities

6. **Authority expansion for interactions**: When entities need to interact across boundaries, temporarily assign both to one server

7. **Persistence layer as crash recovery**: EntityGraph/database acts as source of truth for recovery

### What to Avoid

1. **Distributed physics across boundaries**: World's Adrift proved this is devastatingly complex. Minimize cross-boundary physics.

2. **Relying on determinism**: No production system uses deterministic lockstep for server meshing.

3. **Ignoring the O(n^2) problem**: Interest management is not optional at scale.

4. **Tight coupling to middleware**: World's Adrift died partly because SpatialOS changed its API. Own your networking code.

### Open Questions (Unsolved)

1. **Cross-boundary lag compensation for FPS**: No production system has documented a complete solution for hit registration across server boundaries in fast-paced combat.

2. **Physics simulation at boundaries**: Objects straddling a boundary (e.g., a vehicle half on each server) remain extremely difficult.

3. **Authority transfer latency during combat**: The transfer window creates a brief period where hit registration may be degraded.

4. **Scaling past ~2000 concurrent players in a single space**: Star Citizen tested 2000 but hasn't shipped it. The O(n^2) problem becomes severe.

---

*This document compiles public technical information as of March 2026. Many systems are actively evolving.*
