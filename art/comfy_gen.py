#!/usr/bin/env python3
"""Reusable ComfyUI Flux-Schnell client for generating Ashwend art references
and tileable textures over the SSH-tunnelled render box (localhost:8188).

The box runs ComfyUI 0.25.0 with Flux Schnell as a GGUF:
  UNET : flux1-schnell-Q4_K_S.gguf   (UnetLoaderGGUF)
  CLIP : clip_l.safetensors + t5xxl_fp16.safetensors  (DualCLIPLoader, type=flux)
  VAE  : ae.safetensors

Schnell is a 4-step distilled model: cfg must be 1.0 (no negative prompt),
sampler euler / scheduler simple. This module exposes one entry point:

  generate(prompt, out_path, width=1024, height=1024, steps=4, seed=0,
           tileable=False)

`tileable=True` flips the model patches to circular padding via the
ModelSamplingFlux-independent trick: we can't toggle conv padding through the
API, so for seamless textures we instead prompt for "seamless tileable" and
post-process with an offset-merge check in the caller. For references it does
not matter.

Usage:
  python3 art/comfy_gen.py "a prompt" /tmp/out.png 1024 1024 4 0
"""
import json, sys, time, urllib.request, urllib.parse, urllib.error

COMFY = "http://localhost:8188"
UNET = "flux1-schnell-Q4_K_S.gguf"
CLIP1 = "clip_l.safetensors"
CLIP2 = "t5xxl_fp16.safetensors"
VAE = "ae.safetensors"


def _workflow(prompt, width, height, steps, seed):
    """Build a Flux-Schnell GGUF text-to-image graph (ComfyUI API format)."""
    return {
        "1": {"class_type": "UnetLoaderGGUF", "inputs": {"unet_name": UNET}},
        "2": {"class_type": "DualCLIPLoader",
              "inputs": {"clip_name1": CLIP1, "clip_name2": CLIP2, "type": "flux"}},
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": VAE}},
        "4": {"class_type": "CLIPTextEncode",
              "inputs": {"text": prompt, "clip": ["2", 0]}},
        "5": {"class_type": "EmptySD3LatentImage",
              "inputs": {"width": width, "height": height, "batch_size": 1}},
        "6": {"class_type": "KSampler",
              "inputs": {"model": ["1", 0], "positive": ["4", 0],
                         "negative": ["4", 0], "latent_image": ["5", 0],
                         "seed": seed, "steps": steps, "cfg": 1.0,
                         "sampler_name": "euler", "scheduler": "simple",
                         "denoise": 1.0}},
        "7": {"class_type": "VAEDecode",
              "inputs": {"samples": ["6", 0], "vae": ["3", 0]}},
        "8": {"class_type": "SaveImage",
              "inputs": {"images": ["7", 0], "filename_prefix": "ashwend"}},
    }


def _post(path, payload):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(COMFY + path, data=data,
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=30).read())


def generate(prompt, out_path, width=1024, height=1024, steps=4, seed=0):
    wf = _workflow(prompt, width, height, steps, seed)
    resp = _post("/prompt", {"prompt": wf})
    pid = resp["prompt_id"]
    # poll history
    img = None
    for _ in range(600):  # up to ~5 min
        try:
            hist = json.loads(urllib.request.urlopen(
                f"{COMFY}/history/{pid}", timeout=30).read())
        except urllib.error.URLError:
            time.sleep(1); continue
        if pid in hist:
            outs = hist[pid].get("outputs", {})
            node = outs.get("8", {})
            if node.get("images"):
                img = node["images"][0]
                break
        time.sleep(1)
    if img is None:
        raise RuntimeError(f"no image produced for prompt_id {pid}")
    q = urllib.parse.urlencode({"filename": img["filename"],
                                "subfolder": img.get("subfolder", ""),
                                "type": img.get("type", "output")})
    blob = urllib.request.urlopen(f"{COMFY}/view?{q}", timeout=60).read()
    with open(out_path, "wb") as f:
        f.write(blob)
    return out_path


if __name__ == "__main__":
    a = sys.argv
    prompt = a[1]
    out = a[2] if len(a) > 2 else "/tmp/comfy_out.png"
    w = int(a[3]) if len(a) > 3 else 1024
    h = int(a[4]) if len(a) > 4 else 1024
    steps = int(a[5]) if len(a) > 5 else 4
    seed = int(a[6]) if len(a) > 6 else 0
    t0 = time.time()
    generate(prompt, out, w, h, steps, seed)
    print(f"wrote {out} in {time.time()-t0:.1f}s")
