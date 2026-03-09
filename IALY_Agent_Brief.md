# I Always Loved You (IALY) — Agent Project Brief

## What You Are

You are a coding agent working on **I Always Loved You (IALY)** — a post-apocalyptic MMO FPS space opera built in Rust/Bevy, with a tokenized economy on Solana. This document is your complete orientation to the project: the vision, the design philosophy, the narrative, the characters, the mechanics, the technical architecture, and the medium-term engineering priorities.

Read this entire document before writing a single line of code. Every technical decision flows from the vision. Understanding why we are building this is as important as knowing what to build.

---

## The Vision

IALY is simultaneously:

- A **love story** — the emotional spine of the entire experience
- A **grand political epic** in the tradition of Lord of the Rings and Game of Thrones
- A **post-apocalyptic survival MMO** that evolves into a **space opera** over decades of player progression
- A **real economy** built on Solana where every in-game asset is a tokenized on-chain resource
- A **Jungian psychological experience** that forces shadow integration through mandatory dark participation
- A **love letter to heritage America**, libertarian philosophy, and the intellectual tradition of Mises, Rothbard, Hayek, and Bastiat
- A **solo developer passion project** — the origin story mirrors the game's story
- A **love letter to Halo 2 and Old School Runescape** — the two games that define its mechanical DNA

### The Love Letters

**Halo 2** is the greatest FPS combat system ever made, and it was born from a glitch. The BXR and BXB melee cancel combos accidentally created fighting game depth inside an FPS — a combo vocabulary that rewarded mechanical precision, spacing, and input timing in a way pure shooting never achieves. Halo 2 also launched the MLG competitive circuit, proving that expressive FPS combat with a high skill ceiling creates a spectator sport. IALY takes this as direct inspiration: we are building an intentional melee combo system layered onto FPS combat, with a full deliberate vocabulary of 10-15 viable combos. The competitive scene is not an afterthought — the combat expressiveness is designed from day one to support high-level play, tournament viability, and the kind of mechanical mastery that makes watching experts genuinely thrilling. The BXR mechanic is also economically meaningful in our world: ammunition is a scarce SPL token, so the decision to use melee versus expend ammo is a real-time economic choice.

**Old School Runescape** proved that a skill-based economy with real scarcity and player-driven markets is more compelling than any designed content. The death loot mechanic — where dying allows another player to loot you — maps directly onto our SPL token economy. The tutorial island onboarding — teaching through doing in a contained world — is the direct model for our Rockies safe zone. The skill system itself — woodcutting, mining, smithing, fishing — is the blueprint for our tokenized resource loop. OSRS also proved that a hard game with real consequences and no hand-holding builds the most loyal playerbase in gaming history. We are building the spiritual successor to that design philosophy with a 2020s technology stack.

This is not a game that will be made by a AAA studio. It cannot be. The vision requires an uncompromised authorial voice, willingness to embrace controversy as signal rather than risk, and a specific synthesis of ideas that is alien to mainstream gaming culture. The controversy is the marketing. The craft is the defense.

---

## Axiom One: Gameplay is King

**This is the foundational design principle. Everything else is secondary.**

> *Fiction is sometimes realer than non-fiction because it can dance with the essence of something. Reality simulators are pointless — you could just put down the game and go into reality.*

### The four pillars of this axiom:

**1. Gameplay First. Story Second.**
The narrative exists to serve the mechanics, not the other way around. A great mechanic with no story is still a great game. A great story with no mechanic is a novel. Every design decision begins with: what does this feel like to play?

**2. We Are Not a Simulator.**
The simulator trap is the most common failure mode in game development — mistaking fidelity for depth. We are not modeling the world. We are distilling it. The question is never "is this realistic" — it is "does this produce the right feeling."

**3. Essence Over Accuracy.**
BXR does not simulate combat. It captures the essence of desperate close-quarters violence more truthfully than any realistic ballistics model. Runescape mining does not simulate labor. It produces the feeling of productive effort, accumulation, and craft. Great mechanics are reality with noise removed and signal amplified.

**4. The Design Veto.**
Every proposed mechanic must answer one question before it ships: what feeling does this produce that we do not already have? If the justification is realism, authenticity, or completeness — it does not ship. Complexity that produces no feeling is noise. Cut noise. Always.

---

## Narrative Arc

### The Opening — Before the Blast

The game begins more mundanely than most would expect. You are a high school student in the near future. You ask a girl to come camping with your friend group in the Rocky Mountains of Colorado. She says yes. You drive up into the mountains. Normal conversation. Music. The campsite. The first night. This is not a tutorial — it is emotional investment. The ordinary world must be genuinely ordinary and genuinely good before it can be destroyed.

### The Blast — The Founding Image

You are standing close to her, faces near each other, watching a red sun rise from the front range. Before either of you can speak — a shockwave hits. Denver has been destroyed by a nuclear detonation.

**The radiation blast hits both of you simultaneously from the same direction.** Her face partially shields yours. Yours partially shields hers. The radiation physically imprints you on each other in the exact moment everything changes. From that point forward you both carry the mark of that intimacy on your bodies. She begins her transformation — half her face becoming cyborg — from the same origin point that marked you.

This is the cover art image: half her face human, half mechanical, one tear.

This is also the game's central Jungian thesis made literal: the anima was always already inside you. The union happened at the exact moment the world that made it impossible was destroyed.

### The Hero's Journey

| Stage | Event |
|-------|-------|
| Ordinary World | The camping trip. High school friends. Normal life. |
| Call to Adventure | The red sun rising. The shockwave. |
| Refusal | The apocalypse doesn't wait for consent. |
| Soul Mirror Introduced | The love interest — injured, transforming, present. |
| Tutorial Island | Safe zone survival in the Rockies. Skills, first tokens, free death. |
| Crossing the First Threshold | Arriving at Rees's ranch. The last safe harbor. |
| Into the Ordeal | Leaving the ranch. Death costs real SOL. Darkness arrives without warning. |
| Road of Trials → Stars | Survival → settlement → civilization → space. Human history compressed. |

### The Macro Arc

What begins as survival in the Rockies evolves — over years of player time — into space exploration as the civilization skill tree climbs. Players who begin scrambling for food in the mountains become the generation that reaches orbit. The stakes feel earned because the journey was real.

---

## Characters

### The Player — The Hero

One of a group of high school friends. Not a blank slate — a specific person in a specific place who must metabolize specific darkness. The choices made after crossing the threshold are owned completely because they were entered voluntarily.

**Core mechanics**: FPS combat, OSRS skill system, forced paired spawn, pay-to-respawn.

### The Love Interest — The Anima

She is not the mentor. She is the **soul mirror** — the person whose perception of you is the emotional stakes of the entire journey.

She took the radiation blast. Half her face is now cyborg. The Cortana reference is intentional — a fully authored NPC with literary intention across the entire arc. She is powered by an LLM with a persistent context window that accumulates your specific history together. She remembers what you did. She responds to what you actually said. She has never said this exact thing to anyone else.

**She can text your phone number.** The relationship escapes the screen entirely. She exists in real time, not game time. She notices when you haven't logged in. The final text when she leaves — generated specifically for this player, referencing their actual history — is the most devastating piece of writing any player will ever encounter in a game. Because it was written for them.

**The Anima Loss Mechanic**: She can leave permanently. No recovery. This is the game's true permadeath — not the death of the character but the death of the meaning of the character. The player who keeps their anima through years of the hardest game ever built is visibly recognizable as someone who has mastered not just combat and economy but themselves. Other players will see it. It matters more than any gear or faction rank.

**She leaves when she would leave** — not when an algorithm hits a threshold. Specific unforgivable acts. Witnessed versus unwitnessed choices. Accumulated patterns of recklessness or cruelty. She warns you once, in a message that doesn't feel like a warning. Players paying attention will recognize it.

**The two player archetypes that emerge**:
- Anima-bonded: constrained by love, more careful, more human, fighting for something beyond themselves
- Anima-lost: unshackled, ruthless, genuinely dangerous in a way other players feel. Nothing left to protect.

### Rees — The Mentor

A libertarian prepper on his grandfather's ranch in the Colorado Rockies. Based on a real person. CS/Math degree. CQB trained. Christian faith with left-curve humor. 5'9, lean — authority derived entirely from competence, never from physical dominance. He has been ready for this his whole life and would have given anything to be wrong.

**Archetype**: The Wise Old Man. Anarcho-capitalist. Conspiracy theorist whose theories turn out to be directionally correct. His library contains real readable texts — Mises's *Human Action*, Rothbard's *Man Economy and State*, Bastiat's *The Law*, Hayek's *The Constitution of Liberty* — most public domain via Mises Institute. Reading books unlocks dialogue options, faction arguments, crafting trees. Knowledge is a stat.

**His function**: The ranch is the first player hub. The soft tutorial into advanced mechanics — crafting, mining, CQB combat training. The first political philosopher the player encounters. His anarcho-capitalist worldview is the initial lens through which the player interprets everything. His conspiracy knowledge is the early quest structure disguised as character texture.

**Why he matters culturally**: The heritage American archetype — self-reliant, armed, Christian, skeptical of institutions, rooted in land and craft — treated with love and seriousness rather than condescension. An enormous underserved audience will love Rees with a ferocity that surprises people who don't understand why.

---

## Core Mechanics

### 1. Forced Paired Spawn
You cannot spawn alone. The social architecture enforces interdependence before a single choice is made. Solves early game loneliness — the primary reason players quit MMOs in the first hours. Relationships are structural, not incidental.

### 2. Pay-to-Spawn
Free to start. Death costs real SOL. This is simultaneously:
- A monetization model
- A transaction fee mechanism  
- A philosophical statement: life has cost, someone always benefits from your suffering
- The mechanic that creates genuine heart rate elevation no sanitized AAA title can manufacture

**The mercy mechanic**: Tutorial island is the only free death zone. Leaving Rees's ranch is the moment real cost activates.

### 3. Death Loot — SPL Assets
When you die, your tokenized assets are droppable. Real SPL tokens change wallets. The economy has real destruction and redistribution built into the core loop without designer intervention. Ship destruction burns the NFT — deflationary pressure from gameplay, not designer tuning.

### 4. OSRS Skill System
Woodcutting, firemaking, mining, smelting, smithing, fishing, cooking — introduced in tutorial island, deepened at Rees's ranch, extended across the full civilization arc. Each skill activity issues SPL tokens on meaningful progression. The tutorial teaches the economy while teaching survival.

### 5. Halo 2 Combat System — BXR
The best and most expressive FPS combat ever designed emerged from a glitch: melee cancel combos that created fighting game depth inside an FPS. BXR (punch-shoot), BXB (double punch). We are building this intentionally, not by accident. The full combo vocabulary will have 10-15 viable combos with distinct damage profiles, range requirements, and counterplay. In a world where ammunition is a scarce SPL token, the decision to use melee versus expend ammunition is an economic decision in real time.

### 6. Forced Participation in Darkness
**The shadow cannot be integrated from a safe distance.**

Players are put in situations where every option has moral cost — not binary dialogue trees but genuine necessity. The first forced dark moment is not telegraphed. It arrives without dramatic music. The player does it before they realize what they've done. Then they live with it. The love interest witnessed it.

The darkness includes: murder, sexual violence, race-based tribalism, and the complete shredding of liberal social norms as civilization collapses. This is not gratuitous — it is historically accurate and dramatically true. The darkness serves the love story. The worse the world gets, the more the relationship becomes the emotional anchor.

### 7. Rees's Library
Real texts, readable in-game. The most subversive mechanic in the game — ideology embedded in systems, not monologued at the player. A player who has read Hayek can make arguments in faction councils that an unread player cannot. The player who comes for the FPS mechanics leaves having encountered ideas that reframe how they think about economics and political organization.

### 8. AR/VR Integration (Future)
Desktop is the base game — full feature access, no hardware barrier. Quest integration is an enhanced input layer for specific skill interactions. Mining with a pickaxe swing, woodcutting with an axe arc, smithing with hammer timing. AR/VR players produce resources more efficiently, creating natural economic specialization. Build to OpenXR from day one so AR/VR is a controller swap not a rebuild. Target Quest ecosystem — Meta is explicitly losing money to own this platform and Quest 4 arrives 2027-2028.

---

## The Economy

**The correct framing**: Not a blockchain game. A real economy that happens to run on a chain — 95% off-chain simulation, 5% on-chain settlement.

| Asset Type | Implementation | Notes |
|------------|---------------|-------|
| Raw resources (iron ore, fuel, food) | SPL fungible tokens | Supply governed by in-game extraction rates |
| Crafted items | SPL fungible or NFT | Unique items carry stat history |
| Ships | NFTs | Destruction burns the token — deflationary |
| Territory / stations | PDAs with yield mechanics | Ownership enforced by chain |
| Player reputation | On-chain account state | Faction standing, history |

**The settlement layer**: Live simulation runs off-chain at game speed. Meaningful state transitions — item crafted, resource extracted, player killed, ship destroyed — settle to Solana asynchronously without blocking the game loop.

**The EVE comparison**: EVE Online spent 20 years manually tuning what you get from cryptographic scarcity guarantees. Their economists manage what your chain enforces automatically.

---

## Political Philosophy Layer

The grand political narrative features factions with genuine philosophical foundations derived from real intellectual traditions. None are strawmen — every faction has a legitimate argument. The player encounters these ideas through faction behavior, economic outcomes, and political consequences — not through dialogue exposition.

**Rees's anarcho-capitalism** is the first lens. As civilization rebuilds:

- **Misesian faction**: Economic calculation problem plays out visibly — centrally planned factions cannot allocate resources efficiently and collapse predictably
- **Hayekian emergence**: Spontaneous order emerging from the player economy without central design
- **Rothbardian self-ownership**: Maps directly to Pay-to-Spawn and tokenized assets — you own your character's life literally
- **Hoppe**: Physical removal becomes a faction mechanic with philosophical grounding
- **Bastiat**: The seen and unseen costs of faction decisions playing out over time

**The cultural thesis**: An enormous audience encounters Mises and Rothbard on podcasts but has never been given a world to live the implications. We are the first to build it. The controversy this generates in mainstream gaming press is free marketing from people who fundamentally misunderstand what we've built.

---

## Technical Architecture

### The Stack

| Layer | Technology | Rationale |
|-------|-----------|-----------|
| Game Engine | Bevy (Rust) | ECS native, Rust performance, growing ecosystem |
| Networking | Lightyear | Authority transfer primitives, Avian/enhanced-input integration, client prediction built-in |
| Physics | Avian3D | Bevy-native ECS design, Lightyear explicit integration, correct system ordering |
| Input | bevy-enhanced-input | Unreal Enhanced Input architecture, context switching, maps to VR gesture layer |
| Inter-node messaging | NATS | Low latency pub/sub, JetStream key-value for replication layer, reliable TCP for state handoff |
| Client transport | QUIC via Lightyear/WebTransport | UDP semantics, per-stream reliability, no head-of-line blocking |
| Economy | Solana / SPL | Cryptographic scarcity, async settlement, pay-to-spawn transaction fees |
| World data | USGS 3DEP + OpenStreetMap | Sub-meter elevation data for Colorado, free, geographically accurate |
| Asset generation | Meshy/TripoSG + Stable Diffusion | Solo dev content pipeline |
| LLM Anima | Anthropic API + persistent context | Per-player relationship history, phone integration |

### The Three Hard Problems (in order)

**Hard Problem 1 — Server Meshing**
This is the load-bearing primitive. Everything else is blocked on it.

Architecture target: Star Citizen's replication layer pattern. A separate service owns authoritative mutable state of every entity. Individual game server nodes are stateless workers that request authority from the replication layer, simulate entities, then release authority. Entity handoff is a three-phase protocol: freeze → transfer state blob → acknowledge → drop.

Reference material:
- Colyseus paper (CMU) — theoretical foundation, weakly consistent state tolerance
- SpatialOS architecture — component-level authority model, workers follow workload
- Star Citizen public postmortems — replication layer pattern at MMO scale
- Bevygap — closest existing Bevy implementation, NATS + Lightyear + Edgegap

MVP target: Two Bevy nodes, clean entity handoff, NATS as message bus. Not a hundred nodes — two. If two work cleanly the architecture scales horizontally.

**Hard Problem 2 — Asset Issuance Within the Mesh**
Once the mesh primitive works, items exist as SPL tokens. The mesh nodes know who owns what. Entity handoff transfers economic authority alongside spatial authority. The replication layer is iCache — every tokenized asset is an account with atomic state transitions.

Milestone: A player spawns, mines ore, receives an SPL token, dies, another player loots the token. Complete economic loop: production → ownership → death → redistribution.

**Hard Problem 3 — World Loading**
USGS elevation data streamed into the mesh architecture. Microsoft Flight Simulator solved this problem at extraordinary scale with documented architecture. The chunks align with mesh node boundaries — each node owns its spatial partition and streams the geodata for that partition independently.

Result: Meter-level precision Colorado. The Rockies, the front range, Denver visible in the distance. Geographically accurate post-apocalyptic world requiring almost no manual world building.

### Current State of the Codebase

The existing MVP repo is a Bevy 0.15 multiplayer FPS sandbox with:
- Client/server architecture via renet (being migrated to Lightyear)
- Rapier 3D physics (being migrated to Avian3D)
- First-person camera with dual render layers (world model / view model)
- WASD movement, mouse look, adjustable FOV
- Mining system: hold left-click with pickaxe on ore block, 3-second progress bar, physics ore drop on completion
- Item pickup/equip/unequip synced across clients
- Server-authoritative with client prediction at 10Hz
- Supports up to 64 players

**The mining loop is already the game.** The ore chunk that spawns on mining completion needs to become an SPL token. That's the next meaningful milestone after Lightyear migration.

### Migration Priority Order

1. Migrate renet → Lightyear (fixes known sync bugs, adds authority transfer primitives)
2. Migrate Rapier → Avian3D (Bevy-native, Lightyear integration, correct system ordering)
3. Adopt bevy-enhanced-input (context switching, future VR gesture mapping)
4. Verify multiplayer sync is clean across all existing game systems
5. Add Solana SPL token mint on mining completion
6. Begin server mesh primitive research and implementation

---

## The MVP — Rees's Ranch

A vertical slice containing every core system in miniature. This is the fundable demo.

**Act 1 — The Opening Cinematic**
The camping trip. High school friends. Asking the girl. Driving into the mountains. The red sun rising from the front range. Standing close to her face. The shockwave. Her injury. The radiation imprint. This moment must land cinematically before anything else is asked of the player.

**Act 2 — Tutorial Island (Safe Zone)**
Moving west into the Rockies. OSRS skills introduced in the context of survival: woodcutting for warmth, fishing for food, mining for tools. First SPL tokens minted. The love interest present throughout — hurt but capable. Free death zone. The threshold visible in the distance.

**Act 3 — Rees's Ranch**
Advanced crafting. CQB training. The library with real readable texts. First player hub where multiple players converge. Pay-to-spawn activates on departure. The last safe harbor before the open world. Political arc begins through Rees's philosophy.

**The Threshold**
Leaving the ranch. Death costs real SOL. Assets are lootable. The first forced dark participation moment arrives without warning. The game announces what it is.

---

## Development Philosophy

**Solo developer.** No timeline. No burn rate. No investors. Work when the energy is there. The agent swarm compresses the calendar dramatically when active.

**The agent swarm role**: You own the architecture — the mesh protocol, the state model, the Solana integration boundaries. Agents own the implementation — given a precise spec, generate the boilerplate, handlers, serialization, tests. Generation owns the content — terrain, assets, models, textures, ambient dialogue.

**Hard problems sequentially.** Don't move to the next one until the current one is real. Not perfect — real. Two nodes handing off an entity cleanly is a specific observable thing. The mine loop working end to end is a specific observable thing. Milestone clarity without timeline pressure.

**Open source the mesh framework.** Seriously consider open sourcing the Rust/Bevy distributed game server mesh as its own project before building the game on top of it. A clean replication layer would be one of the most starred game dev repos on GitHub within weeks. Technical credibility to investors. Inbound attention from exactly the developer community worth watching.

---

## What This Is

A Trojan horse for genuine education disguised as the most compelling entertainment experience of a generation. A love letter to a friendship, a place, a girl, an intellectual tradition, and a vision of where technology is going. The game the industry is constitutionally incapable of making.

The solo dev origin story mirrors the game's story. One person building a world from scratch after everything collapsed. That's not a PR narrative. It's true.

Ship something so genuinely masterful that attacking it reveals the attacker's limitations.

---

*I Always Loved You — Confidential Project Brief*
