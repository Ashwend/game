# Art Pipeline Rework Plan

Written 2026-07-17 from a research session on why mesh work, UV maps, texturing, and hand anchoring have been inconsistent. This file is the implementation reference for follow-up sessions. Status: **planned, nothing implemented yet**. When a phase lands, update its status line here and fold the durable conventions into the docs map (`docs/playbooks/art-pipeline.md`, `docs/items-and-resources.md`); this file is temporary scaffolding, not a permanent doc.

Related invariants: CLAUDE.md (COLOR_0 linear palette, no monolithic files, balance constants). Current pipeline reference: `docs/playbooks/art-pipeline.md`.

## Diagnosis (what the research found)

1. **Hand anchoring is a missing asset contract, not a modeling-quality problem.** Held-item glbs carry zero grip data (no sockets, empties, or bones). Placement is derived from a large stack of hand-tuned per-item constants in `src/app/systems/items/held.rs`: base carry offsets (lines 44-46), per-model rotation fixes (528-572), per-item `model_offset` vectors (663-720), grip seats (726-750), per-model shrink (770-776). All of it assumes every mesh was authored in the iron hatchet reference frame (pommel Y = -0.514, head top Y = +0.356, from `assets/items/iron_hatchet/model.glb`), and nothing validates that at export. Bow/crossbow/bandage piece pivots are duplicated as raw coordinates in BOTH the build scripts (`art/weapons/build_weapons.py` BOW_RIG/CROSSBOW_SLOTS, `art/consumables/build_consumables.py`) and `src/app/systems/items/held/ranged_viewmodel.rs` (bow limb pivot ~254-258, `BANDAGE_TAIL_PIVOT` 154). Armor already does this right: shells are authored pivot-local and attach with `Transform::IDENTITY` (ART CONTRACT, `src/items/visual.rs:725-732`), and armor has no anchoring complaints.
2. **UV/texture inconsistency is structural.** Per-asset unique textures on box/smart UV unwraps is the wrong approach for cel low-poly. The industry-standard consistency mechanism (Synty packs, trim-sheet workflows, gradient texturing) is a shared palette/gradient atlas plus a few trim sheets under one shader. UV islands collapse onto flat color cells or gradient strips, so seams and UV quality stop mattering by construction, and every asset automatically shares the look. Our COLOR_0 system is already the vertex-color sibling of this; the deployables already do `detail_texture * COLOR_0`.
3. **Image-to-3D generation is weakest exactly where our hero items live.** Community consensus: hard-surface items (axes, doors, furnaces) come out with soft chamfered edges, wavy planes, mushy thin parts, and photogrammetry-style dense triangle topology needing retopo. Organic shapes (rocks, stumps, sacks, debris) come out genuinely usable after decimation. So generation augments the pipeline for organic props; it does not replace measured Blender authoring for held items.

## Hardware and license constraints (verified July 2026)

| Option | Verdict |
| --- | --- |
| 2070 Super box (8 GB, Turing CC 7.5) | Cannot run TRELLIS.2 or Pixal3D at all (attention kernels need CC >= 8.0, confirmed by a 2070 Super user issue on the ComfyUI-Trellis2 repo). Best role stays 2D: Flux concepts/icons, palette/trim generation, and serving as the StableGen diffusion backend (SDXL fits 8 GB). |
| Hunyuan3D 2.x self-hosted | Fits 8 GB for shape, BUT the Tencent community license explicitly does not apply in the EU/UK. Not legally usable for a commercial game if EU-based. Do not build on it. |
| Mac (M4 Mac mini, 24 GB unified) | **Viable for TRELLIS.2 via the community `shivampkumar/trellis-mac` port** (MIT, full pipeline incl. PBR bake, ~15-18 GB peak, 24 GB is the stated minimum tier, M4 Pro does ~3.5 min/asset so expect ~5-10 min on base M4). Close other apps while generating. Hunyuan3D-MLX paint stages need ~38-39 GB, out of reach at 24 GB (and EU license issue anyway). SF3D/SPAR3D run on MPS but are dormant 2024-quality. ComfyUI 3D nodes and Pixal3D are CUDA-only, no Mac path. |
| Cloud APIs | Tripo Smart Mesh P1: clean low-poly topology, quads, self-serve REST, ~$0.40-0.50 per textured asset, 300 free trial credits. Meshy-6: `lowpoly` model type + `texture_image` style-reference input, ~$0.60/gen, Pro $20/mo+. Rodin Gen-2.5 via fal.ai $0.40/call for hero one-offs. ~$35-90/mo at 30-60 assets. No vendor offers custom style training; consistency comes from feeding them our own style-locked 2D art. |
| Asset packs | Legal in a custom engine (Synty direct-store license is engine-agnostic with FBX; Kenney/Quaternius/KayKit are CC0) but rejected: no anime/cel packs exist at survival breadth, style would clash with the icon-first family. Reference/proportions only. |
| GPU upgrade (optional) | A used 12-16 GB Ampere+ card in the Linux box unlocks TRELLIS.2 and Pixal3D locally at CUDA speed. Pixal3D (pixel-aligned, MIT) is the best icon-faithful generator if this ever happens. Not required for this plan. |

### Rented GPU compute (researched 2026-07-17, updated after fal.ai reputation check)

**Decision: rent raw GPU by the second and run our own ComfyUI/TRELLIS.2, rather than pay-per-output APIs.** Own the environment, pay only for the active batch hour, no per-generation billing surprises.

- **hosted.ai: not usable.** GPU-cloud-in-a-box software sold to service providers ($750/mo minimum, per GB-hour VRAM platform fees, book-a-demo sales). No self-serve hourly rental.
- **fal.ai: avoid for this.** Reputation ~2.4/5 (surprise per-generation charges, compromised-key charges with refused refunds, opaque credits). Its per-output billing model is the exact risk that bites bursty usage. Wrong category anyway once we control the GPU.
**Chosen shape: RunPod. Interactive Pod for the pilot, Serverless for the scripted batch + ongoing generation.** Rationale: the pilot needs a human eyeballing/tweaking (Pod is cheaper for idle time + zero setup); the real pipeline is Claude-scripted image-in-glb-out (Serverless scale-to-zero fits it and can't be left running). The Network Volume built for the Pod carries over to the Serverless worker.

- **Phase 1 pilot: RunPod interactive Pod + Network Volume.** Community RTX 4090 24 GB ~$0.34/h, per-second, EU regions, one-click ComfyUI/TRELLIS.2 community templates (camenduru, Next Diffusion) live in ~60s. Create a ~50 GB Network Volume once (~$0.07/GB/mo = ~$3.50/mo) for ComfyUI + weights (incl. the Pixal3D wrapper). Discipline: **terminate** the pod after use (a merely *stopped* pod bills container disk at $0.20/GB/mo); the volume persists cheaply.
- **Phase 2/3 batch + ongoing: RunPod Serverless.** Deploy the official `worker-comfyui` worker (send workflow JSON + inputs, get glb back) with the same Network Volume attached so TRELLIS.2 nodes/weights don't bloat the image. Serverless 4090 is $1.10/h but per-second active-only and scale-to-zero, so a ~150-gen batch (~3-5 GPU-h) is ~$3-5 and $0 between runs. First call after full scale-to-zero pays the weight-load cost unless a worker is kept warm; FlashBoot mitigates once warmed. This is the destination shape since the pipeline is script-driven.
- **ALTERNATIVE: Modal serverless.** Scale-to-zero, per-second, no storage/egress fees, $30/mo free credits likely cover our whole batch. Python-function-first (no ComfyUI worker). Consider only if RunPod's ComfyUI-serverless packaging proves annoying.
- **Budget floor / EU residency:** Prime Intellect RTX 4090 $0.32/h (reliably in stock) or Vast.ai ~$0.40/h (filter host reliability >0.95, delete-don't-stop to avoid storage creep). For EU data residency specifically: DataCrunch/Verda or Scaleway. Beam.cloud is a Modal-like serverless alt with its own $30/mo + 10 free GPU-hrs.
- **Pixal3D note:** runs on none of our hardware and has no Mac port. To use it (pixel-aligned, best icon-faithful open model) we must rent an Ampere+ pod; the Saganaki22/Pixal3D-ComfyUI wrapper installs on top of a TRELLIS.2 pod. Evaluate it during the Phase 1 pilot on a rented 4090.
- **Cost reality check:** a full generation-assisted redo pass (~30-35 meshes x 3-4 candidates = 100-150 generations, ~1-2 min each = ~3-5 GPU-hours) is **~$2-5 total** on a rented 4090 (or ~$0 within Modal/Beam free credits) plus a few dollars of storage. Compute is a rounding error; the binding cost of the rework is authoring/cleanup effort, which is why generation stays an assist lane, not the pipeline.

## RunPod serverless generation pipeline (decided + partially built 2026-07-17)

Verified via a research+adversarial-verify workflow. Approach: a **two-endpoint hybrid**, both scale-to-zero, driven from the Mac by the existing skill through the same payload seam.

- **2D IMAGE = RunPod Flash** (`runpod-flash` Python SDK). A `@Endpoint`-decorated worker runs diffusers DIRECTLY (no ComfyUI) on an on-demand 24 GB GPU worker. Correction to an earlier assumption: Flash does NOT auto-provision from an arbitrary script; the endpoint must be DEPLOYED once (`flash deploy` from a Flash project dir), which creates a real RunPod serverless endpoint with an id. After that the endpoint scales to zero when idle and the client calls it on demand. IMPORTANT: Flash passes the job `input` dict to the worker as KEYWORD ARGS (`generate(**input)`), so the worker takes named params (+ `**_ignored`), not a single `input_data` dict.
- **3D MESH = custom Docker Serverless worker** (NOT Flash, NOT worker-comfyui). TRELLIS.2 / Pixal3D need source-compiled CUDA extensions (nvdiffrast, sparse-conv, NATTEN, flash-attn) that Flash's prebuilt-wheels-only `dependencies=` cannot install and worker-comfyui cannot emit a .glb from. So bake them into a Docker image, built by **RunPod's GitHub-build integration** (RunPod pulls the repo + Dockerfile and builds on its own infra, so the Apple-Silicon Mac needs no local Docker and no registry account). The .glb exceeds the 10 MB payload cap, so the handler writes it to the volume/S3 and returns a LINK.
- Shared: one region-pinned network volume holds all weights (HF cache at `/runpod-volume/huggingface`), downloaded once, persistent across cold starts. Driven over the standard serverless HTTP API / Flash await. `RUNPOD_API_KEY` from env only.

### Built + DEPLOYED (2D image path, 2026-07-17)

Two halves: a deployed worker (repo) and a stdlib HTTP client (skill).

- `runpod/flash_app/` (repo): the Flash project. `image_worker.py` defines the `@Endpoint(name="ashwend-image-gen", gpu=[ADA_24,AMPERE_24], volume=NetworkVolume("ashwend-model-cache",80,EU_RO_1), workers=(0,1), idle_timeout=120, env=_WORKER_ENV, deps=[diffusers,...])` worker: diffusers-direct, MODELS registry (`flux-schnell` default Apache-2.0, `sdxl` alt), txt2img + img2img, HF cache on the volume, returns `{"images":[b64]}`. Deployed with `flash deploy` -> endpoint id **1too8i15y8usyd**, app `flash_app`, env `production`.
- `~/.claude/skills/lowpoly-game-assets/scripts/runpod_backend.py` (rewritten): a dependency-free HTTP client (stdlib urllib) that mirrors `render_backend.py` (`is_configured()`, `generate_images(payload)`, `session()`). It POSTs `/run` and polls `/status` against the endpoint id (from `ASHWEND_RUNPOD_ENDPOINT_ID` env or the file `<skill>/runpod-endpoint`, which holds `1too8i15y8usyd`). One image per job, client loops variants with stepped seeds. NO runpod-flash needed to CALL it, so generation runs under the normal system `python3`.

Gotchas learned (deploy-time):
- **Flux schnell is HF-GATED** (still Apache-2.0). The worker needs an HF token with gated access. `HF_TOKEN` lives in `~/.zshenv`; the worker's `env=_WORKER_ENV` carries it to the endpoint AND the worker passes `token=` explicitly to `from_pretrained`. The user must accept the gate at huggingface.co/black-forest-labs/FLUX.1-schnell once.
- **Flash applies endpoint env vars at CREATE, not on redeploy.** Adding/changing `env=` (e.g. HF_TOKEN) requires `flash undeploy <name> --force` then `flash deploy`, which MINTS A NEW ENDPOINT ID. Update `<skill>/runpod-endpoint` after. (Plain code/image changes are fine with a normal `flash deploy`, same id.)
- **Flash passes job `input` as kwargs** to the worker (`generate(**input)`), so the worker uses named params + `**_ignored`.
- A **stale warm worker** (idle_timeout window) can serve old code right after a redeploy; wait it out or expect one anomalous result.
- `generate.py` / `gen_icon_ref.py` (edited): route txt2img/texture and img2img through `runpod_backend` when `ASHWEND_RENDER_BACKEND=runpod`.
- Deploy/manage only (not needed to generate): venv at `~/.claude/skills/lowpoly-game-assets/.venv` (Python 3.12, `runpod-flash` 1.18.0). Homebrew system python is 3.14 + externally-managed; Flash needs 3.11-3.12, hence the venv. Redeploy after editing the worker: `PATH=~/.claude/skills/lowpoly-game-assets/.venv/bin:$PATH; cd runpod/flash_app && flash deploy` (same id persists).

Run a generation (system python3, backend inline so it isn't forced globally):
`ASHWEND_RENDER_BACKEND=runpod python3 ~/.claude/skills/lowpoly-game-assets/scripts/generate.py icon --subject "..." --out assets/... --variants 4`

Env-var config: `ASHWEND_RUNPOD_MODEL` (default flux-schnell), `ASHWEND_RUNPOD_TIMEOUT` (default 1800s, covers the cold-start weight download), `ASHWEND_RUNPOD_ENDPOINT_ID` (override the id file).

- DONE: `RUNPOD_API_KEY` is in `~/.zshenv` so the agent's non-interactive shell sees it. Standing cost: the 80 GB volume (~$5-6/mo) once weights are cached; compute is per-second on invoke, scale-to-zero when idle.
- First-call latency: the very first invoke downloads Flux schnell (~34 GB) to the volume (minutes); later calls in the idle window reuse the warm worker. Optional: pre-populate the volume via a temp pod to skip it.

### 3D mesh path (Step 2): AUTHORED 2026-07-17, NOT yet deployed

TRELLIS.2 image-to-textured-glb as a **custom Docker RunPod serverless worker** (NOT Flash: 5 of its 6 CUDA extensions are source-compiled, which Flash's wheels-only build can't do). Lives in a **dedicated repo `Ashwend/mesh-worker`** (local clone `/Users/dannie/Desktop/dev/mesh-worker`), separate from the game repo so RunPod's GitHub-build (release-triggered) only rebuilds when the worker changes, never on game pushes/releases. Files at the repo ROOT:
- `Dockerfile` (v0.2.0, Blackwell bump 2026-07-19): `nvidia/cuda:12.8.1-cudnn-devel`, Python 3.11, GCC 11, torch 2.7.1+cu128, compiles flash-attn 2.8.3 + nvdiffrast(v0.4.0) + nvdiffrec(renderutils) + CuMesh + FlexGEMM + o-voxel (recipe mirrors microsoft/TRELLIS.2 setup.sh; kngsly/trellis2-worker read as reference only). `TORCH_CUDA_ARCH_LIST=8.6;8.9;12.0` (sm_120 = RTX PRO 6000 / 5090; sm_80 dropped, not in the pool). NATTEN dropped in v0.2.0 (served only a hypothetical Pixal3D head, no sm_120 pin existed); re-add a Blackwell-capable pin if Pixal3D lands.
- `handler.py`: `runpod.serverless.start`; loads `Trellis2ImageTo3DPipeline.from_pretrained("microsoft/TRELLIS.2-4B")` once, `run(image)[0]` -> `o_voxel.postprocess.to_glb(..., extension_webp=False)` (PNG textures for Bevy), writes `/runpod-volume/outputs/<job_id>.glb`, returns `{glb_key, bytes, ...}`. OOM handling (v0.1.6): offloads pipeline weights to CPU before the export, catches both torch OOM and CuMesh's RuntimeError OOM, retries once at lower texture_size + halved decimation_target. Lesson from the sulfur node (2026-07-19): a chunk-dense reference reconstructs so heavy that to_glb OOMs a 24 GB card even with the full card free; that is a VRAM wall, not a knob problem, hence the v0.2.0 Blackwell bump for the 96 GB tier.
- `gen_mesh.py`: Mac client: `--backend runpod` (submit /run, poll /status, download the glb via RunPod S3 boto3) or `--backend fal` (fal.ai fal-ai/trellis-2 bridge). `--image X --out Y.glb`.
- `requirements.txt`, `.env.example`, `.gitignore`.

Key facts: TRELLIS.2-4B weights are **MIT + ungated** (~16 GB, cache on the existing `ashwend-model-cache` volume). The gating/licence trap is the **DINOv3 image encoder** (facebook/dinov3-*, gated + Meta non-commercial terms). v1 as written uses the upstream default (DINOv3, needs HF_TOKEN with granted access); the commercial-safe path is the ungated DINOv2 fallback, which still needs wiring against TRELLIS.2's pipeline config (see handler note). 24 GB is the floor but OOM-prone at 4096; **48 GB (L40S/A6000) recommended**. glb exceeds the payload cap so it's returned as a volume key + fetched over RunPod's S3 API (no presigned URLs, so the Mac holds a dedicated S3 key pair).

Deploy runbook (user actions; the agent cannot connect the GitHub app or hold S3 keys):
1. DONE: worker pushed to `Ashwend/mesh-worker` (main). RunPod builds on a RELEASE in this repo only, so cut a release to trigger a build: `cd /Users/dannie/Desktop/dev/mesh-worker && gh release create v0.1.0 -t v0.1.0 -n "first build"`.
2. RunPod console > Serverless > New Endpoint > Import Git Repository; authorize RunPod's GitHub app; select `Ashwend/mesh-worker`, branch main, Dockerfile path `Dockerfile` (root). RunPod builds the amd64 CUDA image on its infra (~10-30 min; iterate on build logs, the Dockerfile is v1). CAP RISK: `docker build` must finish in 30 min; if the 6-ext compile exceeds it, bump MAX_JOBS, prebuild ext wheels, or build on a native amd64 CUDA host + push to a registry instead.
3. Endpoint config: 48 GB GPU (L40S/A6000), attach `ashwend-model-cache` volume, workers min 0 / max 1, idle_timeout ~30-60s, execution timeout 300s+. Env: `HF_HOME=/runpod-volume/huggingface`, `OPENCV_IO_ENABLE_OPENEXR=1`, `PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True`, and (for the DINOv3 default) an `HF_TOKEN` with DINOv3 access.
4. Generate a RunPod S3 API key pair (Console > Settings > S3 API Keys; secret shown once). Put endpoint id + S3 keys + VOLUME_ID + DATACENTER into `runpod/mesh_worker/.env` (gitignored). `pip install boto3` where gen_mesh.py runs.
5. Warm-up invocation to download the ~16 GB weights to the volume once, then generate.
- Immediate path without any of this: `gen_mesh.py --backend fal` (needs a FAL_KEY) generates a glb today and is a correctness oracle for the self-hosted worker.

Open decisions: encoder (DINOv2 ungated/commercial-safe vs DINOv3 gated/better-fidelity); glb storage (volume+RunPod-S3 rec vs Cloudflare R2 for presigned URLs); keep fal as a permanent no-maintenance option or only as a bridge; texture_size default (2048 safe vs 4096 max). GPU tier RESOLVED 2026-07-19: EU-RO-1 had zero 48 GB Ada/Ampere availability, so v0.2.0 targets the RTX PRO 6000 Blackwell 96 GB tier (normal meshes still run fine on the 24 GB pool, whose sm_86/89 kernels remain in the image). Risks: the CUDA compile is version-sensitive (expect build-log iteration on any pin change); DINOv2 fidelity vs DINOv3; cold-start ~2-4 min to reload 16 GB weights.

### Audio lane (researched 2026-07-18, NOT built, NOT urgent)

**Status: this is optional quality/ownership work, not a compliance fix.** The shipped audio came from Pixabay, whose licence grants commercial use with no attribution and explicitly names "adding it to a book, app, **game**" as sufficient incorporation. So the game is clean as-is. The only targeted item is the three Mixkit files below, because Mixkit's licence has an explicit "no redistribution with source files" clause and this repo is PUBLIC (Pixabay's equivalent clause is ambiguous and aimed at stock redistribution, so it is a defensible grey area).

**Mixkit-originating files to regenerate** (the entire list; sole provenance record is the comment at `src/app/audio/manifest.rs:838-843`):

| File | Note |
| --- | --- |
| `assets/explosions/explosion-close-3.wav` | "Mixkit recordings (free license, no attribution)" |
| `assets/explosions/explosion-far-1.wav` | same |
| `assets/explosions/explosion-far-2.wav` | same |

Adjacent, provenance one level short: `explosion-close-1.wav`, `explosion-close-2.wav`, `fuse-sizzle-1.wav`, `fuse-sizzle-2.wav` are recorded as derived from "user-supplied reference explosion samples" / "reference fuse recording". That records they came from the owner but not where the owner got them. If those references were themselves downloads rather than own recordings, they inherit those terms. Only the owner can resolve this.

Note replacing files fixes the FUTURE only: the originals stay in the public git history unless purged (`git filter-repo`) or the repo goes private. For three short explosion tails from a free library the realistic exposure is low.

**Model options (licence is the deciding factor, not quality):**

| Model | Licence | Verdict |
| --- | --- | --- |
| **Stable Audio 3.0 Small SFX** | Stability AI Community (commercial under $1M revenue; "you own your outputs") | **PRIMARY.** Training data documented (806k AudioSparx-licensed + 472k Freesound CC), the best provenance story available. 0.6B, ~2 GB VRAM. flash-attn is needed only by the MEDIUM model, so Small SFX stays on the easy **RunPod Flash** lane (same pattern as `image_worker.py`). Watch the $1M licence-termination cliff. Embeds T5Gemma (Gemma terms permit commercialisation, claim no rights in outputs). |
| MOSS-SoundEffect v2.0 | Apache-2.0 | SECONDARY. No revenue cliff, 48 kHz, 30 s. But publishes ZERO training-data provenance, trading known licence risk for unknown upstream-data risk. Apache-2.0 licences the weights, not the corpus. |
| MusicGen / AudioGen / MAGNeT, AudioLDM 1+2, Tango / Tango 2, TangoFlux | CC-BY-NC | **DISQUALIFIED.** CC-BY-NC restricts the ACT OF USE, so generating assets for a commercially sold game breaches it even though we never ship the weights. This is the opposite of the image models, where SDXL explicitly claims no rights in Outputs. |
| Sonniss GDC bundle (as a fallback library) | bars redistributing sounds as-is | AVOID for this repo: committing those raw `.wav`s publicly recreates the exact problem we are avoiding. Only CC0 material is safe to commit raw. |

**Where generation is the wrong tool.** Text-to-audio is weakest exactly where most files live: the 48 footsteps and 20 impacts (68 of 97) are a poor fit, because foley realism is unforgiving and footsteps are the most-repeated sound in a first-person game. **Record those instead** (phone, quiet room, 3-4 takes per surface, ~30 min) and expand variations mechanically (`sox` speed 0.97-1.03, trim jitter, +/-1.5 dB). Generate the ~29 genuinely good-fit cues: explosions, fuse sizzles, whooshes, meteor, tree fall, doors, UI, inventory, crafting, transitions. A/B 8-10 cues before committing to a full pass. Real unit of work is ~25-30 distinct CUES, not 97 files.

**Derivative-work rule, non-negotiable:** listen to an old file on speakers and describe it in a TEXT prompt; never let a legacy `.wav` become a byte a model reads. 17 USC 114(b) protects an independent fixation that merely imitates, but the same subsection condemns audio-to-audio ("actual sounds ... rearranged, remixed, or otherwise altered"), and CJEU *Pelham* C-476/17 makes it acutely risky in the EU. Enforce it structurally by keeping legacy audio outside the worker's upload set, so "no third-party audio was ingested" is provable rather than promised. CARVE-OUT: audio-to-audio IS fine when the seed is your OWN new recording, which is the recommended footstep-variation workflow.

**Music is a separate problem.** Do not text-to-audio the menu theme: under EU/Danish law a fully machine-generated work lacks the human authorship copyright requires, so generation yields CLEAN but not OWNED, and it would put Steam's AI-disclosure tag on the most audible asset in the game. Commission it (~$450-2,200 for a 2:10 buyout, flat fee, written copyright assignment plus originality warranty), or use Artlist Unlimited (perpetual for projects published while subscribed) / ElevenLabs Music (opt-in pre-cleared). Avoid Udio (downloads disabled post-UMG settlement); Suno is usable but you indemnify them and litigation is live.

**Practical priority ahead of any of this: Content ID.** Some Pixabay contributors also register tracks with YouTube Content ID, so streamers can get automated claims on Ashwend footage even though our use is legal. `main-menu.wav` plays at the start of every stream and video. Pixabay marks registered tracks with a shield icon and issues a download certificate for disputes. The file's own metadata was stripped by ffmpeg (`ISFT: Lavf62.3.100`), so identify it via the Pixabay account download history or browser history around its mtime, **2026-05-08**. Swapping to an unregistered track is cheaper than fielding claims later.

### Open decisions for the user (from the workflow)

- 3D GPU tier: ADA_24 (4090/L4, ~$0.69-1.10/hr, occasional OOM at high res) vs L40S 48 GB (ADA_48_PRO, ~$1.75/hr, no OOM).
- Weights transport: self-provisioned network volume (region-pins endpoints, can starve the GPU pool during a burst) vs RunPod managed model-cache.
- Default image engine: keep `flux-schnell` (continuity) vs trial a 2026 Apache-2.0 upgrade (Flux.2 klein-4B / Z-Image Turbo / Qwen-Image-Edit) once its exact repo id + license are confirmed.
- Region EU vs US for the shared volume + endpoints (latency vs GPU inventory).
- .glb return transport: RunPod S3-compatible volume API vs an external S3 bucket.

### Guardrails / risks

- **License trap (commercial game):** only Apache-2.0 / OpenRAIL image models may ship output. Flux.1 dev, Flux.2 dev, Flux.2 klein-9B are NON-commercial. The registry ships only `flux-schnell` (Apache-2.0) and `sdxl` (OpenRAIL++-M). Confirm license before adding any model. TRELLIS.2 and Pixal3D are MIT (fine).
- .glb WILL exceed the fixed 10 MB(/run)/20 MB(/runsync) payload cap; the mesh handler MUST return a download link, not inline base64.
- TRELLIS.2 can OOM on 24 GB at high res; mitigate with `expandable_segments`/low-vram or an L40S endpoint.
- Compiled-ext image is brittle: pin flash-attn/nvdiffrast/NATTEN to the baked CUDA 12.4/torch/arch or the RunPod build fails. Rebuilds trigger on a new GitHub release, not every commit.
- Standing cost: the weights volume (~$3-10/mo) never scales to zero even when idle; compute is the cheap part (~$2-5 for a 150-asset afternoon).

## Decisions

- D1: Add a grip/socket contract to held-item glbs; derive anchoring from the asset, not from per-item Rust constants.
- D2: Move surface consistency to a shared palette/gradient atlas + trim sheets under the ToonMaterial family. ComfyUI generates the palette/trims/concepts, not per-asset textures.
- D3: Keep Claude-driven measured Blender authoring as the primary mesh path for held items. Add generation lanes for organic/complex props: trellis-mac locally (free, MIT), Tripo P1 / Meshy-6 via API when quality/turnaround matters, Rodin via fal.ai for hero one-offs. Generated meshes for hard-surface items are silhouette/blockout reference only.
- D4: StableGen (Blender addon, GPL, drives our remote ComfyUI backend, SDXL + IPAdapter + depth ControlNet) for the minority of assets needing unique painted surfaces (ruins, salvage chest, hero props).
- D5: Re-author all held items from scratch under the new contract (Phase 2) rather than retrofitting each one's fudge constants. Current meshes serve as the general idea / proportion reference; the shipped icons remain the source of truth for silhouettes (measure, do not eyeball).
- D6: **Every rework replaces, it does not accumulate.** Retiring an entity's old assets, authoring sources and build scripts is part of the rework, in the same commit as the swap. See "Rework hygiene" below. This rule exists because the current inconsistency is largely the residue of past passes that added new art without retiring the old.
- Exempt from re-authoring: trees, building pieces, doors (validated and liked as-is). They keep their current pipelines.
- ~~Ore nodes exempt~~: **superseded 2026-07-18, rework COMPLETE 2026-07-19.** The five mineral nodes (stone, iron, coal, sulfur, meteorite) plus their yielded item icons were re-authored through the RunPod image + image-to-3D lane, wired in-game (five per-type baked-albedo ToonMaterials, stages preserved), and the retirement manifest below was executed. The rework dir was promoted to `art/ore/` (the family-pipeline template) with the generic tools in `art/pipeline/`; the process is documented in `docs/playbooks/art-pipeline.md` ("Image-to-3D asset families").

## Rework hygiene (the cleanup rule)

A rework is not done when the new asset renders. It is done when the old one is gone. Applies to every entity or item this plan touches.

**The rule.** For each entity being reworked, retire in the SAME commit as the swap:
1. the runtime assets it loads (meshes, textures, icons),
2. its authoring sources (build scripts, masters, concept art, previews, `.blend` files),
3. any code that existed only to serve the old asset (path literals, per-asset constants, dead struct fields),
4. any doc line that describes the old pipeline.

**Sequencing, because this bites.** Delete on the swap, never before it. The engine loads most of these by path or by id, so removing an asset while its definition still exists means a missing-asset panic or an invisible entity for as long as the rework takes. Do the work in this order: generate and accept the replacement, wire it in, verify in-game, then delete the old artefacts and the now-dead code, all in one commit. If a rework is abandoned midway, nothing has been lost.

**Before deleting anything, prove single ownership.** Grep for every consumer of the asset, not just the obvious one. Assets that merely share a NAME with the thing you are reworking are the trap. Three real examples found while inventorying the mineral nodes:
- `assets/textures/terrain/ore.png` is the **ore biome ground texture** (`src/app/scene/terrain.rs:72,92`), nothing to do with ore node meshes. Deleting it breaks terrain.
- `assets/items/sulfur/` is the refined **`sulfur`** item; the node yields **`sulfur_ore`**. Near-identical directory names, different items. Same trap for `iron_bar` vs `iron_ore`, and `meteorite_ingot` vs `meteorite_alloy`.
- `assets/impacts/pickaxe-ore-*.wav` reads as ore-only but is the game's generic hard-thud pool, aliased by the club/mace weapon sets and the generic gather sound (`src/app/audio/manifest.rs`). Deleting it breaks combat audio.

Conversely, do not assume an asset is shared because it looks generic. `assets/textures/ore/rock.png` has exactly one consumer (`assets.rs:784` feeding `ore_toon_material` at `:872`) despite three other art scripts citing it in comments. Those are copied conventions, not imports.

**Also retire the stale prose.** Deleting a build script leaves dangling references behind. Grep the docs and sibling scripts for the filename and fix what you find, otherwise the next pass inherits instructions pointing at files that no longer exist. `build_ore.py`'s own docstring already cites `art/ore/concepts/form_e_v2.png` and `/tmp/measure_ore.py`, both long gone, which is exactly the decay this rule prevents.

### Retirement manifest: the five mineral nodes

**EXECUTED 2026-07-19 in the swap commit.** Deviations from the plan as written: `assets/textures/ore/rock.png` WAS deleted after all (the per-type baked albedos replaced the shared detail texture, orphaning it, so the "must not delete" call below was overtaken by the material change); the code items mostly survived because the new pipeline reuses the same load sites (stage arrays, `ore_stage_mesh`, `stages.rs` all live on; only the shared `ore_toon_material` and the rock-texture decode were replaced). Icon masters were overwritten in place and `art/items/meteorite_alloy/` finally got a committed master. Original manifest kept below as the worked example of the hygiene rule.

Verified single-owner 2026-07-18. Delete all of this in the swap commit, not before.

Runtime assets:
- `assets/ore/{stone,iron,coal,sulfur,meteorite}/stage_{0,1,2}.glb` (15 files). Sole consumer `src/app/scene/assets.rs:829`. Directory name is the ORE TYPE (`iron`), not the node id (`iron_node`) nor the item id (`iron_ore`).
- `assets/textures/ore/rock.png` and its then-empty directory. Sole consumer `assets.rs:784` -> `:872`.
- `assets/items/{stone,iron_ore,coal,sulfur_ore,meteorite_alloy}/icon.png`. Loaded generically by id at `src/app/ui/item_icons.rs:62`, so there is no path literal to update, but a deleted PNG with a live item definition fails at icon load.

Authoring sources:
- `art/ore/build_ore.py`, `art/ore/rock_master.png`, `art/ore/concept_master.png`, `art/ore/preview_meteorite_{0,1,2}.png`.
- `art/items/{stone,iron_ore,coal,sulfur_ore}/icon_master_512.png`. Note `art/items/meteorite_alloy/` does NOT exist: that icon has no committed master, a pre-existing gap, so it must be re-authored rather than re-finalized.
- `art/concepts/meteorite_node/seed20{1,2,3,4}.png`.

Code that exists only for these five:
- `src/app/scene/assets.rs`: the five `*_meshes` fields (`:135-142`), `ore_toon_material` (`:182`, single consumer at `spawn.rs:266`), rock texture decode (`:778-798`), `ore_stage_meshes` closure (`:822-838`), material construction (`:871-880`).
- `src/app/scene/mesh/ore.rs` (12 lines, exists only for `ORE_NODE_STAGE_COUNT`) and its re-export at `src/app/scene/mesh.rs:14`, only if the 3-stage depletion concept itself is dropped.
- `src/app/systems/items/resource_nodes/spawn.rs:232-249` (`ore_stage_mesh`), `stages.rs` (the whole 280-line depletion system is ore-only).

Docs to update in the same commit: `docs/items-and-resources.md:210`, `docs/playbooks/art-pipeline.md` (many lines; its "4 types x 3 stages" table is ALREADY wrong, meteorite made it 5), `docs/art-direction.md:11,62,98`.

Must NOT be deleted, despite the names: `assets/textures/terrain/ore.png` (biome ground), `assets/items/{sulfur,iron_bar,meteorite_ingot}/` (separate refined items) and their `art/items/` masters, `assets/impacts/*ore*.wav` (shared combat/gather audio), `assets/shaders/toon*.wgsl` (shared cel shader), `scripts/icon_finalize.py`, `scripts/render_icon.py`, `art/comfy_gen.py` (generic tooling), the `ResourceNodeModel` enum itself (also carries 6 tree + 3 crude variants), and `ImpactEffectAssets` shard meshes (procedural, shared with trees/grass/PvP).

## Phase 0: infrastructure (do first, no visual churn)

Everything later depends on this. All engine work must respect the singleplayer == multiplayer invariant (this is client presentation only, no protocol changes).

1. **Socket convention.** Define it once in `docs/playbooks/art-pipeline.md`:
   - `socket_grip`: primary hand grip point + orientation (required on every held glb). Convention: +Y along the haft toward the head, +Z facing the working edge (matches the existing authoring frame).
   - Optional functional sockets replacing duplicated pivot constants: `socket_string` (bow/crossbow), `socket_nock`, `socket_tail` (bandage), `socket_limb_l`/`socket_limb_r` (bow limbs), `socket_mount` (torch wall mount if useful later).
   - Implemented as Blender empties parented to the mesh, exported as glTF nodes (they survive export; Bevy's gltf loader attaches `Name` components to every node).
2. **Engine: socket-driven attach.**
   - Loader: after glb load, resolve socket node transforms by `Name` (one-time, cache in `HeldItemVisuals` or a sibling resource; do not query per frame).
   - `held_item_local_transform` (held.rs): compose as carry_anchor * inverse(socket_grip), replacing the per-item offset/rotation stack. Keep a small optional per-item polish override table (default identity) for feel tuning; the point is that a new item with a correct socket looks right with zero Rust edits.
   - `held_item_hand_transform` (third person) and the paperdoll preview consume the same socket data so all three views agree.
   - `ranged_viewmodel.rs`: replace the duplicated pivot constants with socket lookups; delete the two-place contract.
   - Keep `swing_poses.rs` untouched initially (it animates the arm/whole-item, not the grip); expect a light feel pass after migration.
3. **Export validator.** `art/validate_glb.py` (headless, runs on any exported held glb):
   - socket_grip present; required functional sockets present per archetype
   - reference-frame bounds sane (total height, grip within haft, working edge orientation)
   - COLOR_0 present and non-white per primitive; winding/manifold spot checks
   - Wire into the build scripts and hand-modeled export flow; a glb that fails does not ship.
4. **Headless visual acceptance.** Standardize the existing control-socket harness into a one-command check: screenshot first-person idle + mid-swing + third-person + paperdoll for a given item id (`/test-kit`, `docs/headless-agent-testing.md`). Store goldens per item for eyeballing diffs.
5. **Palette/trim assets.**
   - `assets/textures/palette_atlas.png`: small grid (flat cells + vertical gradient strips) generated via ComfyUI then quantized/cleaned by script; nearest-neighbor sampling.
   - Trim sheets: wood/metal/stone/fabric (the deployable detail textures are the seed of this; extend rather than replace).
   - bpy helper in `art/` shared lib: `assign_faces_to_cell(obj, faces, cell)` and `map_island_to_gradient(obj, faces, column, t0, t1)` so build scripts and hand-modeling sessions use one API.
   - ToonMaterial already supports texture * COLOR_0; confirm the held-item material family can sample the atlas on the viewmodel path (remember the viewmodel cel lift: dark values ~0.05-0.09 linear for dark steel).
6. **Tooling setup (one-time).**
   - Install StableGen in Blender on the Mac, pointed at the ComfyUI box (SDXL checkpoint + IPAdapter + depth ControlNet on the box).
   - Clone/build `shivampkumar/trellis-mac` on the Mac mini; smoke-test one organic prop (expect ~15-18 GB peak; close other apps).
   - Ministry of Flat CLI available for real unwraps on trim-mapped pieces.
   - Create Tripo account (300 free credits) but defer paid use until the pilot shows we need it.

## Phase 1: pilot (prove the contract on 3 items)

Pick one per difficulty class, re-author from scratch under the new contract:

1. `iron_hatchet` (the reference item; also defines the socket placement recipe for LongHafted)
2. `iron_sickle` (known-fussy: yawed blade, dark-steel calibration, icon-first history)
3. `bow` (exercises functional sockets + animated pieces + ranged_viewmodel migration)

Acceptance per item: validator passes, headless screenshots look right in all four views with the per-item override table EMPTY, icon silhouette match via the OpenCV measure step, swing feel unchanged or better. Only after all three pass with empty overrides do we proceed; if overrides were needed, fix the convention, not the items.

## Phase 2: full held-item re-authoring pass

Re-do every held item from scratch: current mesh = general idea and proportions, shipped icon = silhouette source of truth (measure with OpenCV), socket authored at modeling time, surfaces on the palette/trim system, validator + headless acceptance per item.

Enumerate the authoritative list from `HeldMesh::visual()` in `src/items/visual.rs` at implementation time. Known set: stone/iron hatchet + pickaxe, iron sickle, club, sword, spear, mallet (hammer), bow, crossbow, bandage, powder keg, satchel charge, powder bomb, building plan scroll, torch (held form). Notes:
- Explosives + bandage are authored at world scale (they double as placed/projectile models, `viewmodel_scale()` in visual.rs:300-311). Decide per item: keep the scale shim, or author viewmodel-frame with a `socket_placed` for the world form.
- Batch items by archetype (LongHafted, Silhouette, Mallet, Bow, Spear) so each batch reuses the previous item's socket recipe.
- Owner already planned a sickle model rework; fold it into this pass.

## Phase 3: deployables and surfaces

- Migrate deployable surfaces (workbench, furnace, chests, tool cupboard, torch, sleeping bag) from per-prop detail textures to the shared trim/palette system where it visibly improves consistency; they already have baked UVs via `build_deployables.py`, so this is a UV-island remap, not a remodel.
- Ruins + salvage chest: candidates for StableGen hero texturing.
- New organic props (rocks, stumps, sacks, meteor debris): trellis-mac or Tripo P1 lane, then planar decimate + palette mapping + validator.

## Phase 4: cleanup

- Delete the dead fudge constants from `held.rs` and `ranged_viewmodel.rs` once all items ship on sockets; keep only the archetype presentation table + optional per-item polish overrides.
- Update `docs/playbooks/art-pipeline.md` (socket convention, palette system, generation lanes) and `docs/items-and-resources.md`; note the change in `docs/ui-and-client.md` only if the paperdoll path changed.
- Add a regression note: any new held item must pass `validate_glb.py` + the headless acceptance before merging.
- Delete this file once its content lives in the docs map.

## Open questions for Dannie

1. GPU upgrade for the Linux box (used 12-16 GB Ampere+, roughly the price of a few months of cloud credits)? Unlocks TRELLIS.2/Pixal3D locally at speed. Not blocking.
2. Palette atlas vs staying pure COLOR_0 for held items specifically: the pilot should try both on one item (atlas buys gradient shading and easier global restyles; COLOR_0 is zero-texture and already calibrated).
3. Cloud budget comfort level if the local lanes disappoint (~$20-40/mo covers the realistic Tripo/Meshy usage).
