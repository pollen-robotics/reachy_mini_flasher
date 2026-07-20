# ReachyMiniOS Hugging Face mirror

GitHub release downloads are slow for the ~1.6 GB OS image. Each release is
mirrored to the Hugging Face **dataset** `pollen-robotics/reachy-mini-os`
(xet/LFS CDN, much faster). The flasher pulls from there first and falls back to
GitHub.

## Where the CI lives

The mirror workflow + script live in the **`pollen-robotics/reachy-mini-os`**
repo, added via PR:

- PR: https://github.com/pollen-robotics/reachy-mini-os/pull/75
- `.github/workflows/mirror-to-hf.yml`
- `.github/scripts/mirror_to_hf.py`

It runs after "Generate ReachyMini OS Image" (via `workflow_run`, because a
release published by `GITHUB_TOKEN` does not emit a `release` event) and can be
dispatched manually to backfill a tag. Requires a repo secret `HF_TOKEN`.

## What lands on HF

```
<tag>/<image>.zip        # or .img.gz / .img
<tag>/<image>.bmap
<tag>/<image>.info
latest.json              # { tag, image, bmap, image_size }
```

## Flasher side

`src-tauri/src/images.rs` resolves images HF-first:

1. Fetch `https://huggingface.co/datasets/pollen-robotics/reachy-mini-os/resolve/main/latest.json`.
2. Download the referenced `image` (+ `bmap`) from the same base.
3. If HF is unreachable, fall back to the GitHub releases API.

So the app already works before the mirror is populated (it just uses GitHub
until `latest.json` exists).
