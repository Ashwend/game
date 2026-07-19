# Ashwend image-generation worker (RunPod Flash serverless endpoint).
#
# One @Endpoint that runs diffusers DIRECTLY on an on-demand 24 GB GPU worker
# and returns a PNG as base64. Deployed once with `flash deploy`; the skill's
# runpod_backend.py client then calls it on demand and it scales to zero.
#
# Input dict (see runpod_backend.py._build_job):
#   mode: "txt2img" | "img2img"
#   model: registry key (default "flux-schnell")
#   prompt, negative, steps, width, height, seed
#   strength, init_image (base64)   # img2img only
# Returns: {"images": ["<base64 PNG>"]}   (one image per call)
#
# Only COMMERCIALLY-licensed models belong in MODELS: the game ships their
# output. Flux.1 dev, Flux.2 dev, and Flux.2 klein-9B are NON-commercial.
import os

from runpod_flash import Endpoint, GpuGroup, NetworkVolume, DataCenter

# Weights persist here across cold starts (downloaded once). Pin the volume and
# the endpoint to the same datacenter.
DC = DataCenter.EU_RO_1
VOLUME = NetworkVolume(name="ashwend-model-cache", size=80, datacenter=DC)

# FLUX.1-schnell is Apache-2.0 but its HF repo is GATED: the worker needs an HF
# token to download it. The token is read from the DEPLOY shell's env (put it in
# ~/.zshenv as HF_TOKEN) and baked into the endpoint's worker env at deploy time;
# it is never committed to the repo. huggingface_hub picks up HF_TOKEN
# automatically at from_pretrained time.
_WORKER_ENV = {"HF_HUB_ENABLE_HF_TRANSFER": "1"}
if os.environ.get("HF_TOKEN"):
    _WORKER_ENV["HF_TOKEN"] = os.environ["HF_TOKEN"]

MODELS = {
    "flux-schnell": {
        "repo": "black-forest-labs/FLUX.1-schnell",   # Apache-2.0, ungated
        "txt2img_pipe": "FluxPipeline",
        "img2img_pipe": "FluxImg2ImgPipeline",
        "dtype": "bfloat16",
        "guidance": 0.0,
        "max_seq_len": 256,
        "default_steps": 4,
        "uses_negative": False,
        "offload": True,
    },
    "sdxl": {
        "repo": "stabilityai/stable-diffusion-xl-base-1.0",  # OpenRAIL++-M
        "txt2img_pipe": "StableDiffusionXLPipeline",
        "img2img_pipe": "StableDiffusionXLImg2ImgPipeline",
        "dtype": "float16",
        "guidance": 7.5,
        "max_seq_len": None,
        "default_steps": 30,
        "uses_negative": True,
        "offload": False,
    },
}


@Endpoint(
    name="ashwend-image-gen",
    gpu=[GpuGroup.ADA_24, GpuGroup.AMPERE_24],
    datacenter=DC,
    volume=VOLUME,
    workers=(0, 1),          # (0, n) => true scale-to-zero
    idle_timeout=30,         # bursty agent use: keeps a batch's variants warm, minimal idle tail
    env=_WORKER_ENV,         # carries HF_TOKEN (for the gated Flux repo) to the worker
    dependencies=[
        "diffusers", "transformers", "accelerate", "safetensors",
        "sentencepiece", "protobuf", "pillow", "hf_transfer",
    ],
)
async def generate(
    mode: str = "txt2img",
    model: str = "flux-schnell",
    prompt: str = "",
    negative: str = "",
    steps: int = 4,
    width: int = 1024,
    height: int = 1024,
    seed=None,
    strength: float = 0.7,
    init_image=None,
    **_ignored,          # Flash passes the job `input` dict as kwargs; absorb extras
) -> dict:
    import base64
    import io
    import os

    os.environ.setdefault("HF_HOME", "/runpod-volume/huggingface")
    os.environ.setdefault("HF_HUB_ENABLE_HF_TRANSFER", "1")

    import torch
    import diffusers
    from PIL import Image

    spec = MODELS[model]

    # Reuse the loaded pipeline across calls on a warm worker.
    cache = getattr(generate, "_pipes", None)
    if cache is None:
        cache = {}
        generate._pipes = cache
    key = (spec["repo"], mode)
    pipe = cache.get(key)
    if pipe is None:
        cls_name = spec["img2img_pipe"] if mode == "img2img" else spec["txt2img_pipe"]
        cls = getattr(diffusers, cls_name)
        pipe = cls.from_pretrained(
            spec["repo"],
            torch_dtype=getattr(torch, spec["dtype"]),
            token=os.environ.get("HF_TOKEN") or None,   # gated Flux repo
        )
        if spec["offload"]:
            pipe.enable_model_cpu_offload()
        else:
            pipe = pipe.to("cuda")
        pipe.set_progress_bar_config(disable=True)
        cache[key] = pipe

    kwargs = {
        "prompt": prompt,
        "num_inference_steps": int(steps or spec["default_steps"]),
    }
    if spec["guidance"] is not None:
        kwargs["guidance_scale"] = float(spec["guidance"])
    if spec["max_seq_len"]:
        kwargs["max_sequence_length"] = int(spec["max_seq_len"])
    if spec["uses_negative"]:
        kwargs["negative_prompt"] = negative
    if seed is not None:
        kwargs["generator"] = torch.Generator("cpu").manual_seed(int(seed))
    if mode == "img2img":
        img = Image.open(io.BytesIO(base64.b64decode(init_image))).convert("RGB")
        kwargs["image"] = img
        kwargs["strength"] = float(strength)
    else:
        kwargs["width"] = int(width)
        kwargs["height"] = int(height)

    image = pipe(**kwargs).images[0]
    buf = io.BytesIO()
    image.save(buf, format="PNG")
    return {"images": [base64.b64encode(buf.getvalue()).decode()]}


if __name__ == "__main__":
    # Local smoke of the WORKER logic requires a CUDA box + weights; on the Mac
    # this is just an import/interface check.
    print("image_worker: endpoint 'ashwend-image-gen' defined; deploy with 'flash deploy'.")
