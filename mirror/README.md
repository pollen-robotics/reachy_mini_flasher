# ReachyMiniOS Hugging Face mirror

GitHub release downloads are slow for the ~1.6 GB OS image. Each release is
mirrored to the Hugging Face **Storage Bucket** `pollen-robotics/reachy-mini-os`
(xet CDN, much faster and more reliable). A bucket (not a dataset) is used
because we only need the *latest* image, not a versioned history: it is
overwritten in place on every release. The flasher pulls from there first and
falls back to GitHub.

## Where the CI lives

The mirror workflow + script live in the **`pollen-robotics/reachy-mini-os`**
repo, added via PR:

- PR: https://github.com/pollen-robotics/reachy-mini-os/pull/76
- `.github/workflows/generate_img.yml` (the `mirror-to-hf` job)
- `.github/scripts/mirror_to_hf.py`

It runs on version tags as a `mirror-to-hf` job that `needs: build-image`, so it
reuses the built artifact without rebuilding. Requires a repo secret `HF_TOKEN`
with write access to the `pollen-robotics` namespace.

## What lands on HF

The bucket is written at fixed paths (overwrite-in-place, no per-tag history):

```
reachyminios.zip     # or .img.gz / .img (whatever the build produced)
reachyminios.bmap
latest.json          # { tag, image, bmap, sha256 }
```

`sha256` is the digest of the image file; the flasher verifies the download
against it before writing to the eMMC, so a corrupt/partial upload is caught
early (a mismatch triggers a fresh re-download) instead of only at flash time.

Public bucket files are served anonymously at
`https://huggingface.co/buckets/<namespace>/<name>/resolve/<path>` (buckets are
non-versioned, so there is no revision segment).

## Flasher side

`src-tauri/src/images.rs` resolves images HF-first:

1. Fetch `https://huggingface.co/buckets/pollen-robotics/reachy-mini-os/resolve/latest.json`.
2. Download the referenced `image` (+ `bmap`) from the same base.
3. Verify the image against `sha256` from the manifest before flashing.
4. If HF is unreachable, fall back to the GitHub releases API.

So the app already works before the mirror is populated (it just uses GitHub
until `latest.json` exists).
